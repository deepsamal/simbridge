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

use crate::svd;

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
/// A log line's leading virtual timestamp, "[HH:MM:SS.nnnnnnnnn]" (ticks are ns).
/// Host timestamps have 4 decimals, so they never match.
static TS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(\d+):(\d{2}):(\d{2})\.(\d{9})\] ").unwrap());

/// Renode settings simbridge relies on, passed with `--config` so the user's own
/// Renode config is never touched: virtual-time log stamps for traces, and no
/// merging of repeated lines (a polling loop would otherwise collapse to one access).
const RENODE_CONFIG: &str = "[general]\nlog-timestamps-type = Virtual\ncollapse-repeated-log-entries = false\n";

/// Default global quantum: how often Renode synchronises virtual time, and so
/// the resolution of trace timestamps. Renode's own default is 100 us.
const DEFAULT_QUANTUM_US: f64 = 100.0;

fn parse_ts(line: &str) -> Option<f64> {
    let c = TS_RE.captures(line)?;
    let (h, m, s): (f64, f64, f64) = (c[1].parse().ok()?, c[2].parse().ok()?, c[3].parse().ok()?);
    let ns: f64 = c[4].parse().ok()?;
    Some(((h * 3600.0 + m * 60.0 + s) * 1e9 + ns).round() / 1e9)
}

static TRACE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        // "[cpu: 0x...]" with one CPU; multi-core platforms name it ("[cpu1: 0x...]").
        r"\[(cpu\w*): (0x[0-9A-Fa-f]+)\] (Read|Write)(\w+) (?:from|to) (0x[0-9A-Fa-f]+)",
        r"(?: \(([^)]*)\))?,? (?:returned|value) (0x[0-9A-Fa-f]+)"
    ))
    .unwrap()
});
static RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<(0x[0-9A-Fa-f]+), (0x[0-9A-Fa-f]+)>").unwrap());
static BYTE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x([0-9A-Fa-f]{2})\b").unwrap());

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

/// Windows: a job object with KILL_ON_JOB_CLOSE. Renode is assigned to it, and
/// the handle is never closed, so when simbridge exits for any reason (client
/// gone, killed, crashed) Windows closes the handle and kills Renode with it.
/// Without this, a Renode left over from a killed simbridge keeps its log file
/// open and the next instance can't start.
#[cfg(windows)]
mod kill_on_exit {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    struct Job(HANDLE);
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    static JOB: OnceLock<Option<Job>> = OnceLock::new();

    fn job() -> Option<&'static Job> {
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            (ok != 0).then_some(Job(h))
        })
        .as_ref()
    }

    /// Best effort: returns false if the job couldn't be set up.
    pub fn attach(child: &std::process::Child) -> bool {
        match job() {
            Some(j) => unsafe { AssignProcessToJobObject(j.0, child.as_raw_handle() as HANDLE) != 0 },
            None => false,
        }
    }
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
    cpu: String,
    svd: Option<PathBuf>,
}

impl Loaded {
    /// The SVD peripheral name to prefer for a Renode peripheral (presets can
    /// pick one of several SVD views that share a base address).
    fn svd_name<'a>(&self, renode_name: &'a str) -> &'a str {
        crate::presets::find(&self.platform)
            .and_then(|p| p.svd_alias.iter().find(|(r, _)| r.eq_ignore_ascii_case(renode_name)))
            .map(|(_, s)| *s)
            .unwrap_or(renode_name)
    }
}

impl Loaded {
    fn json(&self) -> Value {
        json!({"elf": self.elf.display().to_string(), "platform": self.platform,
               "repl": self.repl, "uart": self.uart, "cpu": self.cpu,
               "svd": self.svd.as_ref().map(|p| p.display().to_string())})
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
    sync_seq: u64,
    /// Global quantum in seconds, if setting it succeeded (trace timestamp resolution).
    quantum_s: Option<f64>,
    /// Virtual time up to which UART output has been handed out (serial / run_until).
    serial_mark: f64,
}

impl Renode {
    /// `workdir` is a parent folder: each simbridge process uses its own
    /// `simbridge-<pid>` subfolder, so several instances (or a leftover Renode)
    /// never fight over the same log or snapshot files.
    pub fn new(workdir: Option<PathBuf>) -> Result<Self> {
        let workdir = workdir
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("simbridge-{}", std::process::id()));
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
            sync_seq: 0,
            quantum_s: None,
            serial_mark: 0.0,
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
        let config = self.workdir.join("renode.config");
        std::fs::write(&config, RENODE_CONFIG).context("writing Renode config")?;
        let mut cmd = Command::new(&self.renode_path);
        cmd.args(["--disable-xwt", "--plain", "--port", &port.to_string()])
            .arg("--config")
            .arg(abs(&config)?)
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
        #[cfg(windows)]
        kill_on_exit::attach(&child);
        let sock = connect_retry(port, Duration::from_secs(60), || matches!(child.try_wait(), Ok(None)))
            .context("Renode monitor did not come up")?;
        sock.set_read_timeout(Some(Duration::from_millis(500)))?;
        self.proc = Some(child);
        self.sock = Some(sock);
        self.read_until_prompt(Duration::from_secs(60))?;
        let log = q(&self.log_path);
        // Flush after every write: Renode's file logger otherwise flushes on a
        // 5 s timer, and peripheral_trace reads this file.
        self.cmd(&format!("logFile {log} true"))?;
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
        let cpu = self.loaded.as_ref().map(|l| l.cpu.clone()).unwrap_or_else(|| "sysbus.cpu".into());
        self.cmd(&format!("{cpu} PC")).ok()
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

    /// The parsed SVD of the loaded machine, if it has one.
    pub fn svd_device(&self) -> Result<Option<Arc<svd::Device>>> {
        match self.loaded.as_ref().and_then(|l| l.svd.as_ref()) {
            Some(p) => Ok(Some(svd::load(p)?)),
            None => Ok(None),
        }
    }

    pub fn loaded_svd_path(&self) -> Option<PathBuf> {
        self.loaded.as_ref().and_then(|l| l.svd.clone())
    }

    pub fn load(
        &mut self,
        elf: &Path,
        platform: &str,
        uart: Option<&str>,
        cpu: Option<&str>,
        svd_arg: Option<&str>,
    ) -> Result<Value> {
        if !elf.is_file() {
            bail!("ELF not found: {}", elf.display());
        }
        let mut warnings: Vec<String> = Vec::new();
        // An explicit svd argument must resolve; a missing preset default only warns.
        let svd_path = match svd::resolve(svd_arg, platform) {
            Ok(p) => p,
            Err(e) if svd_arg.is_none() => {
                warnings.push(format!("no SVD: {e:#}"));
                None
            }
            Err(e) => return Err(e),
        };
        let svd_dev = match &svd_path {
            Some(p) => Some(svd::load(p)?),
            None => None,
        };
        let (repl, repl_arg, uart, cpu) = match crate::presets::find(platform) {
            Some(p) => (
                p.repl.to_string(),
                format!("@{}", p.repl),
                uart.unwrap_or(p.uart).to_string(),
                cpu.unwrap_or(p.cpu).to_string(),
            ),
            None if !platform.to_ascii_lowercase().ends_with(".repl") => bail!(
                "unknown platform {platform:?}: use a preset ({}) or a path to a .repl file",
                crate::presets::names()
            ),
            None => {
                let u = uart.ok_or_else(|| {
                    anyhow!("custom platform needs an explicit uart (e.g. sysbus.usart1)")
                })?;
                let c = cpu.unwrap_or("sysbus.cpu");
                (platform.to_string(), q(Path::new(platform)), u.to_string(), c.to_string())
            }
        };
        self.cmd("Clear")?; // drop all machines (`mach clear` only deselects)
        self.traced.clear();
        self.cmd(&format!("mach create \"{MACHINE}\""))?;
        // `Clear` resets the emulation, quantum included, so set it per load.
        let q_us = std::env::var("SIMBRIDGE_QUANTUM_US").ok().and_then(|v| v.parse::<f64>().ok())
            .filter(|v| *v > 0.0).unwrap_or(DEFAULT_QUANTUM_US);
        self.quantum_s = match self.cmd(&format!("emulation SetGlobalQuantum \"{:.9}\"", q_us / 1e6)) {
            Ok(_) => Some(q_us / 1e6),
            Err(e) => {
                warnings.push(format!("could not set the global quantum (trace timestamp resolution unknown): {e:#}"));
                None
            }
        };
        self.cmd(&format!("machine LoadPlatformDescription {repl_arg}"))?;
        self.cmd(&format!("sysbus LoadELF {}", q(elf)))?;
        // ApplySVD names registers in traces for regions Renode doesn't model.
        // Best effort: field decoding in regs()/svd() works without it.
        let mut apply_ms = None;
        if let Some(p) = &svd_path {
            let t = Instant::now();
            match self.cmd(&format!("sysbus ApplySVD {}", q(p))) {
                Ok(_) => apply_ms = Some(ms(t)),
                Err(e) => warnings.push(format!("Renode ApplySVD failed (decoding still works): {e:#}")),
            }
        }
        let loaded = Loaded { elf: abs(elf)?, platform: platform.to_string(), repl, uart, cpu, svd: svd_path };
        let mut info = loaded.json();
        self.loaded = Some(loaded);
        self.attach_uart()?;
        info["pc"] = json!(self.pc());
        self.serial_mark = 0.0;
        if let Some(q) = self.quantum_s {
            info["time_resolution_s"] = json!(q);
        }
        if let Some(d) = svd_dev {
            info["svd_device"] = json!(d.name);
            info["svd_peripherals"] = json!(d.peripherals.len());
        }
        if let Some(a) = apply_ms {
            info["svd_apply_ms"] = json!(a);
        }
        if !warnings.is_empty() {
            info["warnings"] = json!(warnings);
        }
        Ok(info)
    }

    /// Deterministic reset: rebuild the machine from scratch and reload the ELF.
    pub fn reset(&mut self) -> Result<Value> {
        let l = self.require_loaded()?;
        let traced = self.traced.clone();
        let svd_arg = l.svd.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "none".into());
        let info = self.load(&l.elf, &l.platform, Some(&l.uart), Some(&l.cpu), Some(&svd_arg))?;
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
        if re.is_some() {
            // run_until handed this text out, so serial() starts after it.
            self.serial_mark = self.virtual_time_s()?.unwrap_or(self.serial_mark);
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
        // The text was produced between the previous hand-out and now.
        let t_from = self.serial_mark;
        let t_to = self.virtual_time_s()?.unwrap_or(t_from);
        self.serial_mark = t_to;
        Ok(json!({"uart": l.uart, "text": String::from_utf8_lossy(&data), "bytes": data.len(),
                  "truncated_bytes": truncated + dropped, "t_from": t_from, "t_to": t_to}))
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
    ///
    /// With an SVD loaded, each word is decoded into its register(s) and fields.
    /// `register` (an SVD register name) reads just that register.
    pub fn regs(
        &mut self,
        peripheral: &str,
        count: u32,
        offset: u64,
        register: Option<&str>,
        verbose: bool,
    ) -> Result<Value> {
        self.require_loaded()?;
        let (start, end) = self.peripheral_range(peripheral)?;
        let dev = self.svd_device()?;
        let name = self.require_loaded()?.svd_name(Self::short(peripheral)).to_string();
        let sp = dev.as_ref().and_then(|d| d.find(&name, Some(start)));
        let (offset, count) = match register {
            Some(name) => {
                let sp = sp.ok_or_else(|| {
                    anyhow!("register lookup needs an SVD peripheral matching {peripheral} at 0x{start:08X}")
                })?;
                let r = sp.register(name).ok_or_else(|| {
                    anyhow!("{} has no register {name:?}; try svd(peripheral={:?})", sp.name, sp.name)
                })?;
                (r.offset & !3, r.size.div_ceil(32).max(1))
            }
            None => (offset, count),
        };
        let room = ((end - start + 1).saturating_sub(offset) / 4) as u32;
        let count = count.clamp(1, 64).min(room);
        let mut words = Vec::new();
        for i in 0..count as u64 {
            let off = offset + 4 * i;
            let v = self.cmd(&format!("sysbus ReadDoubleWord 0x{:X}", start + off))?;
            let raw = v.split_whitespace().next().unwrap_or("").to_string();
            let mut w = json!({"offset": format!("0x{off:X}"), "value": raw});
            if let (Some(sp), Ok(val)) = (sp, parse_hex(&raw)) {
                let decoded: Vec<Value> = sp
                    .decode_word(off, val, verbose)
                    .into_iter()
                    .filter(|d| register.is_none_or(|n| d["register"].as_str().is_some_and(|r| r.eq_ignore_ascii_case(n))))
                    .collect();
                if !decoded.is_empty() {
                    w["registers"] = json!(decoded);
                }
            }
            words.push(w);
        }
        let mut out = json!({"peripheral": Self::short(peripheral), "base": format!("0x{start:08X}"),
                             "size": end - start + 1, "words": words});
        if let Some(sp) = sp {
            out["svd_peripheral"] = json!(sp.name);
        } else if let Some(d) = &dev {
            out["note"] = json!(format!("no peripheral at 0x{start:08X} in SVD {}; raw words only", d.name));
        }
        Ok(out)
    }

    /// Grader only: one raw monitor command, output returned as text. Monitor
    /// errors are returned in `output`, not raised, so the grader can inspect them.
    pub fn monitor(&mut self, line: &str) -> Result<Value> {
        let out = self.cmd_t(line, Duration::from_secs(120), false)?;
        Ok(json!({"command": line, "output": out}))
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

    /// Read the log from `start` up to a sync point. Renode's logger hands entries
    /// to a background thread, so the file can lag the simulation slightly. We log
    /// a unique line with the monitor's `log` command and wait for it: entries go
    /// through one ordered queue, so once it's in the file, every access logged
    /// before it is too. Returns the bytes before the sync line, how many bytes to
    /// consume (through the sync line), and whether it appeared within the timeout.
    fn read_log_synced(&mut self, start: u64) -> Result<(Vec<u8>, u64, bool)> {
        self.sync_seq += 1;
        let token = format!("simbridge-sync-{}", self.sync_seq);
        let marker = Regex::new(&format!(r"{}\b", regex::escape(&token)))?;
        self.cmd(&format!("log \"{token}\""))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let mut buf = Vec::new();
            let mut f = File::open(&self.log_path)?; // std opens with FILE_SHARE_READ|WRITE on Windows
            f.seek(SeekFrom::Start(start))?;
            f.read_to_end(&mut buf)?;
            let text = String::from_utf8_lossy(&buf);
            if let Some(m) = marker.find(&text) {
                // Accesses end where the sync line starts; consume through its newline.
                let line_start = text[..m.start()].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let line_end = text[m.end()..].find('\n').map(|i| m.end() + i + 1).unwrap_or(text.len());
                return Ok((text.as_bytes()[..line_start].to_vec(), line_end as u64, true));
            }
            if Instant::now() > deadline {
                return Ok((buf.clone(), buf.len() as u64, false));
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Accesses logged since the previous call (the first call enables tracing).
    pub fn peripheral_trace(&mut self, peripheral: &str, limit: usize, disable: bool, decode: bool) -> Result<Value> {
        self.require_loaded()?;
        let short = Self::short(peripheral).to_string();
        if !self.traced.contains(&short) {
            self.enable_trace(&short)?;
            return Ok(json!({"peripheral": short, "enabled": true, "accesses": [], "total": 0,
                             "note": "tracing enabled; run_for() then call again"}));
        }
        let start = *self.log_offsets.get(&short).unwrap_or(&0);
        let (chunk, consumed, synced) = self.read_log_synced(start)?;
        self.log_offsets.insert(short.clone(), start + consumed);
        // Renode tags lines "usart2:" with one machine, "sb/usart2:" with several.
        let tag = Regex::new(&format!(r"[\] /]{}: \[cpu\w*:", regex::escape(&short)))?;
        let accesses: Vec<Value> = String::from_utf8_lossy(&chunk)
            .lines()
            .filter(|l| tag.is_match(l))
            .filter_map(|l| TRACE_RE.captures(l).map(|c| (parse_ts(l), c)))
            .map(|(t, c)| {
                let mut a = json!({"pc": &c[2], "op": &c[3][..1], "width": &c[4], "offset": &c[5],
                                   "reg": c.get(6).map(|m| m.as_str()), "value": &c[7]});
                if let Some(t) = t {
                    a["t"] = json!(t); // virtual seconds, at most one quantum early
                }
                if &c[1] != "cpu" {
                    a["cpu"] = json!(&c[1]); // which core made the access
                }
                a
            })
            .collect();
        let total = accesses.len();
        if disable {
            self.cmd(&format!("sysbus LogPeripheralAccess sysbus.{short} false"))?;
            self.traced.remove(&short);
        }
        let mut tail = accesses[total.saturating_sub(limit)..].to_vec();
        if decode {
            let base = self.peripheral_range(&short).ok().map(|r| r.0);
            if let Some(dev) = self.svd_device()? {
                let name = self.require_loaded()?.svd_name(&short).to_string();
                if let Some(sp) = dev.find(&name, base) {
                    for a in tail.iter_mut() {
                        let (Some(off), Some(val)) = (
                            a["offset"].as_str().and_then(|s| parse_hex(s).ok()),
                            a["value"].as_str().and_then(|s| parse_hex(s).ok()),
                        ) else { continue };
                        // Trace offsets may be absolute on some Renode versions.
                        let off = match base { Some(b) if off >= b => off - b, _ => off };
                        let d = sp.decode_word(off & !3, val << ((off & 3) * 8), false);
                        if !d.is_empty() {
                            a["decoded"] = json!(d);
                        }
                    }
                }
            }
        }
        let mut out = json!({"peripheral": short, "total": total, "returned": tail.len(), "accesses": tail});
        if let Some(q) = self.quantum_s {
            out["time_resolution_s"] = json!(q);
        }
        if !synced {
            out["warning"] = json!("Renode's log did not catch up within 10 s; the most recent accesses may be missing");
        }
        Ok(out)
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
        self.serial_mark = self.virtual_time_s()?.unwrap_or(0.0);
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

#[cfg(test)]
mod tests {
    use super::TRACE_RE;

    #[test]
    fn sync_marker_is_exact() {
        let m = regex::Regex::new(&format!(r"{}\b", regex::escape("simbridge-sync-7"))).unwrap();
        assert!(m.is_match("[INFO] monitor: Script: simbridge-sync-7"));
        assert!(!m.is_match("[INFO] monitor: Script: simbridge-sync-70"));
    }

    #[test]
    fn trace_line_single_and_multi_core() {
        let one = "[12:11:54.2010]  [INFO] usart2: [cpu: 0x800052E] ReadUInt32 from 0x0 (Status), returned 0xC0";
        let c = TRACE_RE.captures(one).unwrap();
        assert_eq!((&c[1], &c[2], &c[6], &c[7]), ("cpu", "0x800052E", "Status", "0xC0"));
        let two = "[18:38:57.1041]  [INFO] uart: [cpu1: 0x8000030C] WriteUInt32 to 0x0 (TransmitData), value 0x68";
        let c = TRACE_RE.captures(two).unwrap();
        assert_eq!((&c[1], &c[3], &c[5], &c[7]), ("cpu1", "Write", "0x0", "0x68"));
    }
}

#[cfg(test)]
mod ts_tests {
    use super::parse_ts;

    #[test]
    fn virtual_timestamps_parse_and_host_ones_dont() {
        assert_eq!(parse_ts("[00:00:01.000100000] [INFO] usart2: [cpu: 0x8000494] WriteUInt32 ..."), Some(1.0001));
        assert_eq!(parse_ts("[01:02:03.000000005] x"), Some(3723.000000005));
        assert_eq!(parse_ts("[18:38:57.1041]  [INFO] uart: ..."), None);
    }
}
