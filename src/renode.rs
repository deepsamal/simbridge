//! Blocking client for a headless Renode instance driven through its monitor socket.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde_json::{json, Value};

const MACHINE: &str = "sb";
const UART_BUF_MAX: usize = 1 << 20;
const ERROR_MARKERS: &[&str] = &[
    "There was an error",
    "Could not tokenize",
    "No such command",
    "does not provide a field",
    "Could not find",
    "Exception",
];

static PROMPT_RE: LazyLock<regex::bytes::Regex> =
    LazyLock::new(|| regex::bytes::Regex::new(r"\r?\(([^()\r\n]+)\) $").unwrap());
static VT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Elapsed Virtual Time: (\d+):(\d+):(\d+(?:\.\d+)?)").unwrap());
static TRACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\[cpu: (0x[0-9A-Fa-f]+)\] (Read|Write)(\w+) (?:from|to) (0x[0-9A-Fa-f]+)",
        r"(?: \(([^)]*)\))?,? (?:returned|value) (0x[0-9A-Fa-f]+)"
    ))
    .unwrap()
});
static RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(0x[0-9A-Fa-f]+), (0x[0-9A-Fa-f]+)>").unwrap());
static BYTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x([0-9A-Fa-f]{2})\b").unwrap());

/// Built-in platform presets: (name, repl shipped with Renode, console UART).
const PLATFORMS: &[(&str, &str, &str)] = &[
    ("stm32f4", "platforms/cpus/stm32f4.repl", "sysbus.usart2"),
    ("fe310", "platforms/cpus/sifive-fe310.repl", "sysbus.uart0"),
    ("hifive1", "platforms/cpus/sifive-fe310.repl", "sysbus.uart0"),
];

fn abs(path: &Path) -> Result<PathBuf> {
    Ok(std::path::absolute(path)?)
}

/// Quote a host path for the monitor (spaces break the `@path` tokenizer).
fn q(path: &Path) -> String {
    let p = abs(path).unwrap_or_else(|_| path.to_path_buf());
    format!("\"{}\"", p.display().to_string().replace('\\', "/"))
}

fn free_port() -> Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn connect_retry(port: u16, timeout: Duration, mut alive: impl FnMut() -> bool) -> Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(s) => return Ok(s),
            Err(e) if Instant::now() > deadline || !alive() => {
                return Err(anyhow!("could not connect to 127.0.0.1:{port}: {e}"))
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Remove telnet IAC negotiation sequences that Renode's monitor sends.
fn strip_iac(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(buf.len());
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == 0xFF && i + 1 < buf.len() {
            i += if (0xFB..=0xFE).contains(&buf[i + 1]) { 3 } else { 2 };
            continue;
        }
        out.push(buf[i]);
        i += 1;
    }
    out
}

pub fn find_renode() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("SIMBRIDGE_RENODE") {
        return Ok(p.into());
    }
    let exe = if cfg!(windows) { "renode.exe" } else { "renode" };
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let c = dir.join(exe);
            if c.is_file() {
                return Ok(c);
            }
        }
    }
    for c in [r"C:\Program Files\Renode\renode.exe", "/opt/renode/renode"] {
        if Path::new(c).is_file() {
            return Ok(c.into());
        }
    }
    bail!("Renode not found; set SIMBRIDGE_RENODE to the renode executable")
}

/// Connects to a Renode ServerSocketTerminal and buffers everything it sends.
struct UartTap {
    buf: Arc<Mutex<(Vec<u8>, usize)>>, // (data, dropped bytes)
    sock: TcpStream,
}

impl UartTap {
    fn connect(port: u16) -> Result<Self> {
        let sock = connect_retry(port, Duration::from_secs(5), || true)?;
        let buf = Arc::new(Mutex::new((Vec::new(), 0usize)));
        let (mut rd, b) = (sock.try_clone()?, buf.clone());
        thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            while let Ok(n) = rd.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                let mut g = b.lock().unwrap();
                g.0.extend_from_slice(&chunk[..n]);
                if g.0.len() > UART_BUF_MAX {
                    let cut = g.0.len() - UART_BUF_MAX;
                    g.0.drain(..cut);
                    g.1 += cut;
                }
            }
        });
        Ok(Self { buf, sock })
    }

    fn take(&self) -> (Vec<u8>, usize) {
        thread::sleep(Duration::from_millis(50)); // let in-flight bytes land after RunFor
        self.take_now()
    }

    fn take_now(&self) -> (Vec<u8>, usize) {
        let mut g = self.buf.lock().unwrap();
        (std::mem::take(&mut g.0), std::mem::take(&mut g.1))
    }
}

impl Drop for UartTap {
    fn drop(&mut self) {
        let _ = self.sock.shutdown(std::net::Shutdown::Both);
    }
}

#[derive(Clone)]
struct Loaded {
    elf: PathBuf,
    platform: String,
    repl: String,
    uart: String,
}

impl Loaded {
    fn json(&self) -> Value {
        json!({"elf": self.elf.display().to_string(), "platform": self.platform,
               "repl": self.repl, "uart": self.uart})
    }
}

struct Snapshot {
    path: PathBuf,
    loaded: Loaded,
    traced: BTreeSet<String>,
}

pub struct Renode {
    renode_path: PathBuf,
    pub workdir: PathBuf,
    log_path: PathBuf,
    proc: Option<Child>,
    sock: Option<TcpStream>,
    uart: Option<UartTap>,
    loaded: Option<Loaded>,
    traced: BTreeSet<String>,
    log_offsets: BTreeMap<String, u64>,
    snapshots: BTreeMap<String, Snapshot>,
    term_seq: u32,
}

impl Renode {
    pub fn new(workdir: Option<PathBuf>) -> Result<Self> {
        let workdir = workdir
            .unwrap_or_else(|| std::env::temp_dir().join(format!("simbridge-{}", std::process::id())));
        std::fs::create_dir_all(&workdir)?;
        Ok(Self {
            renode_path: find_renode()?,
            log_path: workdir.join("renode.log"),
            workdir,
            proc: None,
            sock: None,
            uart: None,
            loaded: None,
            traced: BTreeSet::new(),
            log_offsets: BTreeMap::new(),
            snapshots: BTreeMap::new(),
            term_seq: 0,
        })
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded.is_some()
    }

    // ---- process / monitor -------------------------------------------------

    fn running(&mut self) -> bool {
        matches!(self.proc.as_mut().map(|p| p.try_wait()), Some(Ok(None)))
    }

    pub fn start(&mut self) -> Result<()> {
        if self.running() && self.sock.is_some() {
            return Ok(());
        }
        let port = free_port()?;
        let mut cmd = Command::new(&self.renode_path);
        cmd.args(["--disable-xwt", "--plain", "--port", &port.to_string()])
            // stdout belongs to the MCP transport: Renode must never inherit it.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd.spawn().with_context(|| format!("spawning {}", self.renode_path.display()))?;
        let sock = connect_retry(port, Duration::from_secs(60), || matches!(child.try_wait(), Ok(None)))
            .context("Renode monitor did not come up")?;
        sock.set_read_timeout(Some(Duration::from_millis(500)))?;
        self.proc = Some(child);
        self.sock = Some(sock);
        self.read_until_prompt(Duration::from_secs(60))?;
        let log = q(&self.log_path);
        self.cmd(&format!("logFile {log}"))?;
        Ok(())
    }

    pub fn stop(&mut self) {
        self.uart = None;
        if let Some(mut s) = self.sock.take() {
            let _ = s.write_all(b"quit\n");
        }
        if let Some(mut p) = self.proc.take() {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = p.try_wait() {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
            let _ = p.kill();
        }
    }

    fn read_until_prompt(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let sock = self.sock.as_mut().ok_or_else(|| anyhow!("monitor not connected"))?;
        let deadline = Instant::now() + timeout;
        let mut raw = Vec::new();
        let mut chunk = [0u8; 65536];
        while Instant::now() < deadline {
            match sock.read(&mut chunk) {
                Ok(0) => bail!("Renode monitor closed the connection"),
                Ok(n) => {
                    raw.extend_from_slice(&chunk[..n]);
                    let clean = strip_iac(&raw);
                    if PROMPT_RE.is_match(&clean) {
                        return Ok(clean);
                    }
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => return Err(e.into()),
            }
        }
        bail!("timed out waiting for Renode prompt")
    }

    fn cmd_t(&mut self, line: &str, timeout: Duration, check: bool) -> Result<String> {
        self.start()?;
        self.sock.as_mut().unwrap().write_all(format!("{line}\n").as_bytes())?;
        let raw = self.read_until_prompt(timeout)?;
        let raw = PROMPT_RE.replace(&raw, &b""[..]);
        let mut text = String::from_utf8_lossy(&raw).replace('\r', "");
        if let Some(i) = text.find(line) {
            text = text[i + line.len()..].to_string(); // drop echo (and any stale prompt)
        }
        let out = text.lines().filter(|l| !l.trim().is_empty()).collect::<Vec<_>>().join("\n");
        let out = out.trim().to_string();
        if check && ERROR_MARKERS.iter().any(|m| out.contains(m)) {
            bail!("`{line}` failed: {}", out.chars().take(500).collect::<String>());
        }
        Ok(out)
    }

    pub fn cmd(&mut self, line: &str) -> Result<String> {
        self.cmd_t(line, Duration::from_secs(120), true)
    }

    // ---- helpers -----------------------------------------------------------

    pub fn virtual_time_s(&mut self) -> Result<Option<f64>> {
        let out = self.cmd("machine ElapsedVirtualTime")?;
        Ok(VT_RE.captures(&out).map(|c| {
            let h: f64 = c[1].parse().unwrap_or(0.0);
            let m: f64 = c[2].parse().unwrap_or(0.0);
            let s: f64 = c[3].parse().unwrap_or(0.0);
            ((h * 3600.0 + m * 60.0 + s) * 1e9).round() / 1e9
        }))
    }

    fn pc(&mut self) -> Option<String> {
        self.cmd("sysbus.cpu PC").ok()
    }

    fn attach_uart(&mut self) -> Result<()> {
        self.uart = None;
        self.term_seq += 1;
        let port = free_port()?;
        let name = format!("sbterm{}", self.term_seq);
        self.cmd(&format!("emulation CreateServerSocketTerminal {port} \"{name}\" false"))?;
        let uart = self.require_loaded()?.uart;
        self.cmd(&format!("connector Connect {uart} {name}"))?;
        self.uart = Some(UartTap::connect(port)?);
        Ok(())
    }

    fn require_loaded(&self) -> Result<Loaded> {
        self.loaded.clone().ok_or_else(|| anyhow!("no machine loaded; call load() first"))
    }

    fn short(peripheral: &str) -> &str {
        peripheral.strip_prefix("sysbus.").unwrap_or(peripheral)
    }

    // ---- operations --------------------------------------------------------

    pub fn load(&mut self, elf: &Path, platform: &str, uart: Option<&str>) -> Result<Value> {
        if !elf.is_file() {
            bail!("ELF not found: {}", elf.display());
        }
        let (repl, repl_arg, uart) =
            match PLATFORMS.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(platform)) {
                Some((_, repl, u)) => (repl.to_string(), format!("@{repl}"), uart.unwrap_or(u).to_string()),
                None => {
                    let u = uart.ok_or_else(|| {
                        anyhow!("custom platform needs an explicit uart (e.g. sysbus.usart1)")
                    })?;
                    (platform.to_string(), q(Path::new(platform)), u.to_string())
                }
            };
        self.cmd("Clear")?; // drop all machines (`mach clear` only deselects)
        self.traced.clear();
        self.cmd(&format!("mach create \"{MACHINE}\""))?;
        self.cmd(&format!("machine LoadPlatformDescription {repl_arg}"))?;
        self.cmd(&format!("sysbus LoadELF {}", q(elf)))?;
        let loaded = Loaded { elf: abs(elf)?, platform: platform.to_string(), repl, uart };
        let mut info = loaded.json();
        self.loaded = Some(loaded);
        self.attach_uart()?;
        info["pc"] = json!(self.pc());
        Ok(info)
    }

    /// Deterministic reset: rebuild the machine from scratch and reload the ELF.
    pub fn reset(&mut self) -> Result<Value> {
        let l = self.require_loaded()?;
        let traced = self.traced.clone();
        let info = self.load(&l.elf, &l.platform, Some(&l.uart))?;
        for p in traced {
            self.enable_trace(&p)?;
        }
        Ok(info)
    }

    pub fn run_for(&mut self, virtual_ms: f64) -> Result<Value> {
        self.require_loaded()?;
        if !(virtual_ms > 0.0) {
            bail!("virtual_ms must be > 0");
        }
        self.cmd_t(
            &format!("emulation RunFor \"{:.9}\"", virtual_ms / 1000.0),
            Duration::from_secs(3600),
            true,
        )?;
        Ok(json!({"pc": self.pc()}))
    }

    /// Advance in `step_ms` virtual-time slices until a stop condition holds or
    /// `timeout_ms` of virtual time has passed. Conditions: UART output (since the
    /// last serial() call) matches `uart_regex`, and/or the 32-bit word at `mem_addr`
    /// (number or ELF symbol) equals `mem_equals`. Stop time is accurate to one step.
    /// UART text seen here is consumed (returned in `text`, not by a later serial()).
    pub fn run_until(
        &mut self,
        uart_regex: Option<&str>,
        mem_addr: Option<&str>,
        mem_equals: Option<u64>,
        timeout_ms: f64,
        step_ms: f64,
    ) -> Result<Value> {
        self.require_loaded()?;
        let re = uart_regex.map(Regex::new).transpose().context("bad uart_regex")?;
        let watch = match (mem_addr, mem_equals) {
            (Some(a), Some(v)) => Some((self.resolve_addr(a)?, v)),
            (None, None) => None,
            _ => bail!("mem_addr and mem_equals must be given together"),
        };
        if re.is_none() && watch.is_none() {
            bail!("give uart_regex and/or mem_addr + mem_equals");
        }
        if !(step_ms > 0.0 && timeout_ms > 0.0) {
            bail!("step_ms and timeout_ms must be > 0");
        }
        let uart = self.require_loaded()?.uart;
        let mut text: Vec<u8> = Vec::new();
        let (mut elapsed, mut steps) = (0.0f64, 0u64);
        let mut hit: Option<Value> = None;
        while elapsed < timeout_ms && hit.is_none() {
            let step = step_ms.min(timeout_ms - elapsed);
            self.cmd_t(
                &format!("emulation RunFor \"{:.9}\"", step / 1000.0),
                Duration::from_secs(3600),
                true,
            )?;
            elapsed += step;
            steps += 1;
            if let Some(re) = &re {
                // UART bytes trail RunFor slightly; give the socket a moment.
                thread::sleep(Duration::from_millis(2));
                text.extend(self.uart.as_ref().map(|u| u.take_now().0).unwrap_or_default());
                if let Some(m) = re.find(&String::from_utf8_lossy(&text)) {
                    hit = Some(json!({"reason": "uart", "matched": m.as_str()}));
                }
            }
            if let (None, Some((addr, want))) = (&hit, watch) {
                let v = parse_hex(self.cmd(&format!("sysbus ReadDoubleWord 0x{addr:X}"))?.trim())?;
                if v == want {
                    hit = Some(json!({"reason": "mem", "addr": format!("0x{addr:08X}"),
                                      "value": format!("0x{v:X}")}));
                }
            }
        }
        let tail = String::from_utf8_lossy(&text[text.len().saturating_sub(4096)..]).to_string();
        let mut out = json!({"hit": hit.is_some(), "elapsed_virtual_ms": (elapsed * 1e6).round() / 1e6,
                             "steps": steps, "resolution_ms": step_ms, "uart": uart,
                             "text": tail, "pc": self.pc()});
        if let Some(h) = hit {
            out["stop"] = h;
        }
        Ok(out)
    }

    pub fn serial(&mut self, max_bytes: usize) -> Result<Value> {
        let l = self.require_loaded()?;
        let (mut data, dropped) = self.uart.as_ref().ok_or_else(|| anyhow!("UART not attached"))?.take();
        let truncated = data.len().saturating_sub(max_bytes);
        if truncated > 0 {
            data.drain(..truncated);
        }
        Ok(json!({"uart": l.uart, "text": String::from_utf8_lossy(&data), "bytes": data.len(),
                  "truncated_bytes": truncated + dropped}))
    }

    fn peripheral_range(&mut self, peripheral: &str) -> Result<(u64, u64)> {
        let out = self.cmd(&format!("sysbus GetRegistrationPoints sysbus.{}", Self::short(peripheral)))?;
        let c = RANGE_RE
            .captures(&out)
            .ok_or_else(|| anyhow!("no bus registration for {peripheral}: {out}"))?;
        Ok((parse_hex(&c[1])?, parse_hex(&c[2])?))
    }

    fn resolve_addr(&mut self, addr: &str) -> Result<u64> {
        let s = addr.trim();
        if let Some(h) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
            return Ok(u64::from_str_radix(h, 16)?);
        }
        if let Ok(n) = s.parse::<u64>() {
            return Ok(n);
        }
        let out = self.cmd(&format!("sysbus GetSymbolAddress \"{s}\""))?;
        parse_hex(out.split_whitespace().next().unwrap_or(""))
    }

    /// Read `count` 32-bit words from the peripheral's bus window. Reads go through
    /// the model, so registers with read side effects behave as if the CPU read them.
    pub fn regs(&mut self, peripheral: &str, count: u32, offset: u64) -> Result<Value> {
        self.require_loaded()?;
        let (start, end) = self.peripheral_range(peripheral)?;
        let room = ((end - start + 1).saturating_sub(offset) / 4) as u32;
        let count = count.clamp(1, 64).min(room);
        let mut words = Vec::new();
        for i in 0..count as u64 {
            let off = offset + 4 * i;
            let v = self.cmd(&format!("sysbus ReadDoubleWord 0x{:X}", start + off))?;
            words.push(json!({"offset": format!("0x{off:X}"),
                              "value": v.split_whitespace().next().unwrap_or("")}));
        }
        Ok(json!({"peripheral": Self::short(peripheral), "base": format!("0x{start:08X}"),
                  "size": end - start + 1, "words": words}))
    }

    pub fn mem(&mut self, addr: &str, length: u32) -> Result<Value> {
        self.require_loaded()?;
        let a = self.resolve_addr(addr)?;
        let length = length.clamp(1, 1024);
        let out = self.cmd(&format!("sysbus ReadBytes 0x{a:X} {length}"))?;
        let hex: String = BYTE_RE.captures_iter(&out).map(|c| c[1].to_ascii_lowercase()).collect();
        Ok(json!({"addr": format!("0x{a:08X}"), "len": hex.len() / 2, "hex": hex}))
    }

    fn log_size(&self) -> u64 {
        std::fs::metadata(&self.log_path).map(|m| m.len()).unwrap_or(0)
    }

    fn enable_trace(&mut self, peripheral: &str) -> Result<()> {
        let short = Self::short(peripheral).to_string();
        self.cmd(&format!("sysbus LogPeripheralAccess sysbus.{short} true"))?;
        self.log_offsets.insert(short.clone(), self.log_size());
        self.traced.insert(short);
        Ok(())
    }

    /// Accesses logged since the previous call (the first call enables tracing).
    pub fn peripheral_trace(&mut self, peripheral: &str, limit: usize, disable: bool) -> Result<Value> {
        self.require_loaded()?;
        let short = Self::short(peripheral).to_string();
        if !self.traced.contains(&short) {
            self.enable_trace(&short)?;
            return Ok(json!({"peripheral": short, "enabled": true, "accesses": [], "total": 0,
                             "note": "tracing enabled; run_for() then call again"}));
        }
        let start = *self.log_offsets.get(&short).unwrap_or(&0);
        let mut chunk = Vec::new();
        let mut f = File::open(&self.log_path)?; // std opens with FILE_SHARE_READ|WRITE on Windows
        f.seek(SeekFrom::Start(start))?;
        f.read_to_end(&mut chunk)?;
        self.log_offsets.insert(short.clone(), start + chunk.len() as u64);
        // Renode tags lines "usart2:" with one machine, "sb/usart2:" with several.
        let tag = Regex::new(&format!(r"[\] /]{}: \[cpu", regex::escape(&short)))?;
        let accesses: Vec<Value> = String::from_utf8_lossy(&chunk)
            .lines()
            .filter(|l| tag.is_match(l))
            .filter_map(|l| TRACE_RE.captures(l))
            .map(|c| {
                json!({"pc": &c[1], "op": &c[2][..1], "width": &c[3], "offset": &c[4],
                       "reg": c.get(5).map(|m| m.as_str()), "value": &c[6]})
            })
            .collect();
        let total = accesses.len();
        if disable {
            self.cmd(&format!("sysbus LogPeripheralAccess sysbus.{short} false"))?;
            self.traced.remove(&short);
        }
        let tail = accesses[total.saturating_sub(limit)..].to_vec();
        Ok(json!({"peripheral": short, "total": total, "returned": tail.len(), "accesses": tail}))
    }

    pub fn snapshot(&mut self) -> Result<Value> {
        let loaded = self.require_loaded()?;
        let id = format!("s{}", self.snapshots.len() + 1);
        let path = self.workdir.join(format!("{id}.save"));
        let t = Instant::now();
        self.cmd(&format!("Save {}", q(&path)))?;
        let save_ms = ms(t);
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.snapshots.insert(id.clone(), Snapshot { path, loaded, traced: self.traced.clone() });
        Ok(json!({"id": id, "save_ms": save_ms, "size_bytes": size}))
    }

    pub fn restore(&mut self, id: &str) -> Result<Value> {
        let (path, loaded, traced) = match self.snapshots.get(id) {
            Some(s) => (s.path.clone(), s.loaded.clone(), s.traced.clone()),
            None => bail!(
                "unknown snapshot {id:?} (have: {})",
                self.snapshots.keys().cloned().collect::<Vec<_>>().join(", ")
            ),
        };
        let t = Instant::now();
        self.cmd(&format!("Load {}", q(&path)))?;
        self.cmd(&format!("mach set \"{MACHINE}\""))?;
        let load_ms = ms(t);
        self.loaded = Some(loaded);
        // Terminals are transient externals: not saved in the snapshot, so re-attach.
        self.attach_uart()?;
        self.traced.clear();
        for p in traced {
            self.enable_trace(&p)?;
        }
        Ok(json!({"id": id, "load_ms": load_ms, "restore_total_ms": ms(t), "pc": self.pc()}))
    }
}

impl Drop for Renode {
    fn drop(&mut self) {
        self.stop();
    }
}

fn parse_hex(s: &str) -> Result<u64> {
    let h = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(h, 16).with_context(|| format!("not a hex number: {s:?}"))
}

pub fn ms(t: Instant) -> f64 {
    (t.elapsed().as_secs_f64() * 10_000.0).round() / 10.0
}
