//! CMSIS-SVD support: parse a device description once, flatten it into a
//! per-peripheral register index, and decode raw register values into fields.
//!
//! Parsing uses `svd-parser` with `expand` on, so `derivedFrom`, `dim` arrays
//! and inherited register properties (size, access, reset) are already
//! resolved. Clusters are flattened here, with offsets made absolute relative to
//! the peripheral base.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use regex::Regex;
use serde_json::{json, Value};
use svd_parser::svd::{
    self, Access, ModifiedWriteValues, ReadAction, RegisterCluster, Usage, ValidateLevel,
};

#[derive(Debug, Clone)]
pub struct EnumVal {
    pub name: String,
    pub value: u64,
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub lsb: u32,
    pub width: u32,
    pub access: Option<Access>,
    pub description: Option<String>,
    pub read_action: Option<ReadAction>,
    pub modified_write: Option<ModifiedWriteValues>,
    /// Values usable when reading (usage read / read-write / unspecified).
    pub read_enums: Vec<EnumVal>,
    /// Values meant only for writing.
    pub write_enums: Vec<EnumVal>,
}

#[derive(Debug, Clone)]
pub struct Register {
    pub name: String,
    /// Byte offset from the peripheral base (clusters flattened).
    pub offset: u64,
    pub size: u32,
    pub access: Option<Access>,
    pub reset: Option<u64>,
    pub description: Option<String>,
    pub read_action: Option<ReadAction>,
    pub modified_write: Option<ModifiedWriteValues>,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Peripheral {
    pub name: String,
    pub base: u64,
    pub group: Option<String>,
    pub description: Option<String>,
    pub registers: Vec<Register>,
}

#[derive(Debug)]
pub struct Device {
    pub path: PathBuf,
    pub name: String,
    pub peripherals: Vec<Peripheral>,
}

static CACHE: LazyLock<Mutex<HashMap<PathBuf, Arc<Device>>>> = LazyLock::new(Default::default);

/// Parse (or fetch from cache) the SVD at `path`.
pub fn load(path: &Path) -> Result<Arc<Device>> {
    let path = std::path::absolute(path)?;
    if let Some(d) = CACHE.lock().unwrap().get(&path) {
        return Ok(d.clone());
    }
    let xml = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let xml = normalize(&xml);
    let cfg = svd_parser::Config::default()
        .expand(true)
        .expand_properties(true)
        .validate_level(ValidateLevel::Disabled); // vendor SVDs are often sloppy
    let dev = svd_parser::parse_with_config(&xml, &cfg)
        .map_err(|e| anyhow!("parsing {}: {e:#}", path.display()))?;
    let d = Arc::new(index(&path, &dev));
    CACHE.lock().unwrap().insert(path, d.clone());
    Ok(d)
}

/// Canonical spellings for the enum-valued SVD tags. Vendors often get the case
/// wrong (Nordic writes `read-writeonce`), and svd-parser rejects anything that
/// isn't spelled exactly as in the schema.
const CANONICAL: &[(&str, &[&str])] = &[
    ("access", &["read-only", "write-only", "read-write", "writeOnce", "read-writeOnce"]),
    ("readAction", &["clear", "set", "modify", "modifyExternal"]),
    ("usage", &["read", "write", "read-write"]),
    (
        "modifiedWriteValues",
        &["oneToClear", "oneToSet", "oneToToggle", "zeroToClear", "zeroToSet", "zeroToToggle",
          "clear", "set", "modify"],
    ),
];

static ENUM_TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"<(access|readAction|usage|modifiedWriteValues)>\s*([^<]*?)\s*</").unwrap()
});

fn normalize(xml: &str) -> std::borrow::Cow<'_, str> {
    ENUM_TAG_RE.replace_all(xml, |c: &regex::Captures| {
        let (tag, val) = (&c[1], &c[2]);
        let canon = CANONICAL
            .iter()
            .find(|(t, _)| *t == tag)
            .and_then(|(_, vals)| vals.iter().find(|v| v.eq_ignore_ascii_case(val)))
            .copied()
            .unwrap_or(val);
        format!("<{tag}>{canon}</")
    })
}

fn index(path: &Path, dev: &svd::Device) -> Device {
    let default_size = dev.default_register_properties.size.unwrap_or(32);
    let peripherals = dev
        .peripherals
        .iter()
        .map(|p| {
            let mut registers = Vec::new();
            if let Some(rc) = &p.registers {
                flatten(rc, 0, default_size, &mut registers);
            }
            registers.sort_by(|a, b| a.offset.cmp(&b.offset).then(a.name.cmp(&b.name)));
            Peripheral {
                name: p.name.clone(),
                base: p.base_address,
                group: p.group_name.clone(),
                description: clean(p.description.as_deref()),
                registers,
            }
        })
        .collect();
    Device { path: path.to_path_buf(), name: dev.name.clone(), peripherals }
}

fn flatten(items: &[RegisterCluster], base: u64, default_size: u32, out: &mut Vec<Register>) {
    for item in items {
        match item {
            RegisterCluster::Register(r) => {
                let fields = r
                    .fields
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|f| {
                        let (mut read_enums, mut write_enums) = (Vec::new(), Vec::new());
                        for ev in &f.enumerated_values {
                            let vals = ev.values.iter().filter_map(|v| {
                                Some(EnumVal {
                                    name: v.name.clone(),
                                    value: v.value?,
                                    description: clean(v.description.as_deref()),
                                })
                            });
                            match ev.usage {
                                Some(Usage::Write) => write_enums.extend(vals),
                                _ => read_enums.extend(vals),
                            }
                        }
                        Field {
                            name: f.name.clone(),
                            lsb: f.bit_range.offset,
                            width: f.bit_range.width,
                            access: f.access.or(r.properties.access),
                            description: clean(f.description.as_deref()),
                            read_action: f.read_action,
                            modified_write: f.modified_write_values,
                            read_enums,
                            write_enums,
                        }
                    })
                    .collect::<Vec<_>>();
                let mut fields = fields;
                fields.sort_by_key(|f| std::cmp::Reverse(f.lsb));
                out.push(Register {
                    name: r.name.clone(),
                    offset: base + r.address_offset as u64,
                    size: r.properties.size.unwrap_or(default_size),
                    access: r.properties.access,
                    reset: r.properties.reset_value,
                    description: clean(r.description.as_deref()),
                    read_action: r.read_action,
                    modified_write: r.modified_write_values,
                    fields,
                });
            }
            RegisterCluster::Cluster(c) => {
                flatten(&c.children, base + c.address_offset as u64, default_size, out);
            }
        }
    }
}

/// Collapse the whitespace/newlines vendors put inside descriptions.
fn clean(s: Option<&str>) -> Option<String> {
    let s = s?.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

fn access_str(a: Option<Access>) -> Option<&'static str> {
    a.map(|a| match a {
        Access::ReadOnly => "ro",
        Access::WriteOnly => "wo",
        Access::ReadWrite => "rw",
        Access::WriteOnce => "w1",
        Access::ReadWriteOnce => "rw1",
    })
}

fn read_action_str(a: Option<ReadAction>) -> Option<&'static str> {
    a.map(|a| match a {
        ReadAction::Clear => "clear",
        ReadAction::Set => "set",
        ReadAction::Modify => "modify",
        ReadAction::ModifyExternal => "modifyExternal",
    })
}

fn mwv_str(m: Option<ModifiedWriteValues>) -> Option<&'static str> {
    m.and_then(|m| match m {
        ModifiedWriteValues::OneToClear => Some("oneToClear"),
        ModifiedWriteValues::OneToSet => Some("oneToSet"),
        ModifiedWriteValues::OneToToggle => Some("oneToToggle"),
        ModifiedWriteValues::ZeroToClear => Some("zeroToClear"),
        ModifiedWriteValues::ZeroToSet => Some("zeroToSet"),
        ModifiedWriteValues::ZeroToToggle => Some("zeroToToggle"),
        ModifiedWriteValues::Clear => Some("clear"),
        ModifiedWriteValues::Set => Some("set"),
        ModifiedWriteValues::Modify => None, // the default: plain write
    })
}

fn insert_opt(m: &mut serde_json::Map<String, Value>, k: &str, v: Option<impl Into<Value>>) {
    if let Some(v) = v {
        m.insert(k.into(), v.into());
    }
}

impl Field {
    pub fn bits(&self) -> String {
        if self.width == 1 {
            format!("{}", self.lsb)
        } else {
            format!("{}:{}", self.lsb + self.width - 1, self.lsb)
        }
    }

    fn extract(&self, value: u64) -> u64 {
        let mask = if self.width >= 64 { u64::MAX } else { (1u64 << self.width) - 1 };
        (value >> self.lsb) & mask
    }

    /// Full description of the field (for the `svd` lookup tool).
    pub fn describe(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("name".into(), json!(self.name));
        m.insert("bits".into(), json!(self.bits()));
        insert_opt(&mut m, "access", access_str(self.access));
        insert_opt(&mut m, "description", self.description.clone());
        insert_opt(&mut m, "read_action", read_action_str(self.read_action));
        insert_opt(&mut m, "write", mwv_str(self.modified_write));
        let ev = |v: &[EnumVal]| -> Value {
            v.iter()
                .map(|e| {
                    let mut m = serde_json::Map::new();
                    m.insert("value".into(), json!(e.value));
                    m.insert("name".into(), json!(e.name));
                    insert_opt(&mut m, "description", e.description.clone());
                    Value::Object(m)
                })
                .collect()
        };
        if !self.read_enums.is_empty() {
            m.insert("values".into(), ev(&self.read_enums));
        }
        if !self.write_enums.is_empty() {
            m.insert("write_values".into(), ev(&self.write_enums));
        }
        Value::Object(m)
    }
}

impl Register {
    pub fn summary(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("offset".into(), json!(format!("0x{:X}", self.offset)));
        m.insert("name".into(), json!(self.name));
        m.insert("size".into(), json!(self.size));
        insert_opt(&mut m, "access", access_str(self.access));
        insert_opt(&mut m, "reset", self.reset.map(|r| format!("0x{r:X}")));
        insert_opt(&mut m, "description", self.description.clone());
        insert_opt(&mut m, "read_action", read_action_str(self.read_action));
        Value::Object(m)
    }

    pub fn describe(&self) -> Value {
        let mut v = self.summary();
        insert_opt(v.as_object_mut().unwrap(), "write", mwv_str(self.modified_write));
        v["fields"] = self.fields.iter().map(Field::describe).collect();
        v
    }

    /// Decode a raw register value. `verbose` adds field descriptions; otherwise
    /// each field is just name, bit range, value and the matching enum name.
    pub fn decode(&self, value: u64, verbose: bool) -> Value {
        let mask = if self.size >= 64 { u64::MAX } else { (1u64 << self.size) - 1 };
        let value = value & mask;
        let mut m = serde_json::Map::new();
        m.insert("register".into(), json!(self.name));
        m.insert("offset".into(), json!(format!("0x{:X}", self.offset)));
        m.insert("value".into(), json!(format!("0x{value:0w$X}", w = (self.size as usize).div_ceil(4))));
        insert_opt(&mut m, "reset", self.reset.map(|r| format!("0x{r:X}")));
        if verbose {
            insert_opt(&mut m, "description", self.description.clone());
        }
        // Reading this register changes it; the value shown was read anyway.
        if let Some(ra) = read_action_str(self.read_action) {
            m.insert("read_side_effect".into(), json!(ra));
        }
        let fields: Vec<Value> = self
            .fields
            .iter()
            .map(|f| {
                let v = f.extract(value);
                let mut fm = serde_json::Map::new();
                fm.insert("name".into(), json!(f.name));
                fm.insert("bits".into(), json!(f.bits()));
                fm.insert("value".into(), json!(v));
                if let Some(e) = f.read_enums.iter().find(|e| e.value == v) {
                    fm.insert("is".into(), json!(e.name));
                }
                if f.access == Some(Access::WriteOnly) {
                    fm.insert("note".into(), json!("write-only: read value is meaningless"));
                }
                if let Some(ra) = read_action_str(f.read_action) {
                    fm.insert("read_side_effect".into(), json!(ra));
                }
                if verbose {
                    insert_opt(&mut fm, "description", f.description.clone());
                }
                Value::Object(fm)
            })
            .collect();
        if !fields.is_empty() {
            m.insert("fields".into(), json!(fields));
        }
        Value::Object(m)
    }
}

impl Peripheral {
    pub fn summary(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("name".into(), json!(self.name));
        m.insert("base".into(), json!(format!("0x{:08X}", self.base)));
        insert_opt(&mut m, "group", self.group.clone());
        insert_opt(&mut m, "description", self.description.clone());
        Value::Object(m)
    }

    pub fn register(&self, name: &str) -> Option<&Register> {
        self.registers.iter().find(|r| r.name.eq_ignore_ascii_case(name))
    }

    /// Registers that live (at least partly) in the 32-bit word at `word_off`.
    pub fn registers_in_word(&self, word_off: u64) -> Vec<&Register> {
        self.registers.iter().filter(|r| r.offset >= word_off && r.offset < word_off + 4).collect()
    }

    /// Decode a 32-bit word read at `word_off` into every register it contains.
    pub fn decode_word(&self, word_off: u64, word: u64, verbose: bool) -> Vec<Value> {
        self.registers_in_word(word_off)
            .into_iter()
            .map(|r| r.decode(word >> ((r.offset - word_off) * 8), verbose))
            .collect()
    }
}

impl Device {
    /// Find a peripheral by bus base address (preferred: Renode and SVD names
    /// often differ) and/or by name. When several share a base (alternate
    /// peripherals), a name match wins.
    pub fn find(&self, name: &str, base: Option<u64>) -> Option<&Peripheral> {
        let by_name = |p: &Peripheral| p.name.eq_ignore_ascii_case(name);
        if let Some(b) = base {
            let at: Vec<&Peripheral> = self.peripherals.iter().filter(|p| p.base == b).collect();
            if let Some(p) = at.iter().find(|p| by_name(p)).or(at.first()) {
                return Some(p);
            }
        }
        self.peripherals.iter().find(|p| by_name(p))
    }

    pub fn require(&self, name: &str) -> Result<&Peripheral> {
        self.find(name, None).ok_or_else(|| {
            anyhow!("peripheral {name:?} not in {} (call svd() with no arguments to list them)", self.name)
        })
    }

    pub fn summary(&self) -> Value {
        json!({"device": self.name, "svd": self.path.display().to_string(),
               "peripherals": self.peripherals.iter().map(Peripheral::summary).collect::<Vec<_>>()})
    }

    /// Regex search (case-insensitive) over peripheral, register and field names
    /// and descriptions.
    pub fn search(&self, pattern: &str, limit: usize) -> Result<Value> {
        let re = Regex::new(&format!("(?i){pattern}")).context("bad query regex")?;
        let hit = |s: &str| re.is_match(s);
        let hit_o = |s: &Option<String>| s.as_deref().is_some_and(hit);
        let mut out = Vec::new();
        'outer: for p in &self.peripherals {
            if hit(&p.name) || hit_o(&p.description) {
                out.push(json!({"peripheral": p.name, "base": format!("0x{:08X}", p.base),
                                "description": p.description}));
            }
            for r in &p.registers {
                if hit(&r.name) || hit_o(&r.description) {
                    out.push(json!({"peripheral": p.name, "register": r.name,
                                    "offset": format!("0x{:X}", r.offset), "description": r.description}));
                }
                for f in &r.fields {
                    if hit(&f.name) || hit_o(&f.description) {
                        out.push(json!({"peripheral": p.name, "register": r.name, "field": f.name,
                                        "bits": f.bits(), "description": f.description}));
                    }
                }
                if out.len() >= limit {
                    break 'outer;
                }
            }
        }
        let truncated = out.len() >= limit;
        out.truncate(limit);
        Ok(json!({"device": self.name, "query": pattern, "matches": out, "truncated": truncated}))
    }
}

/// Where the SVD collections live: `SIMBRIDGE_SVD_DIR`, else the first `svd`
/// directory found next to the executable or one of its ancestors (so
/// `<root>/simbridge/target/release/simbridge.exe` finds `<root>/svd`), else
/// `./svd`.
pub fn svd_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("SIMBRIDGE_SVD_DIR") {
        return Some(PathBuf::from(d));
    }
    let mut starts = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        starts.push(exe);
    }
    if let Ok(cwd) = std::env::current_dir() {
        starts.push(cwd.join("x"));
    }
    for s in starts {
        for dir in s.ancestors().skip(1).take(6) {
            let c = dir.join("svd");
            if c.is_dir() {
                return Some(c);
            }
        }
    }
    None
}

/// Resolve the `svd` argument: `None` -> the platform preset's default (may be
/// absent), `"none"` -> no SVD, a preset name -> its default, otherwise a path
/// (relative paths are tried against the SVD directory first).
pub fn resolve(arg: Option<&str>, platform: &str) -> Result<Option<PathBuf>> {
    let preset = |name: &str| -> Result<Option<PathBuf>> {
        let Some(rel) = crate::presets::find(name).map(|p| p.svd) else {
            return Ok(None);
        };
        let dir = svd_dir().ok_or_else(|| {
            anyhow!("no SVD directory found for preset {name:?}; set SIMBRIDGE_SVD_DIR")
        })?;
        let p = dir.join(rel);
        if !p.is_file() {
            bail!("preset SVD for {name:?} not found at {}", p.display());
        }
        Ok(Some(p))
    };
    match arg.map(str::trim) {
        None | Some("") => preset(platform),
        Some(s) if s.eq_ignore_ascii_case("none") => Ok(None),
        Some(s) => {
            if let Some(p) = preset(s).ok().flatten() {
                return Ok(Some(p));
            }
            let p = PathBuf::from(s);
            if p.is_relative() {
                if let Some(d) = svd_dir() {
                    if d.join(&p).is_file() {
                        return Ok(Some(d.join(p)));
                    }
                }
            }
            if !p.is_file() {
                bail!("SVD not found: {s}");
            }
            Ok(Some(p))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svd_test_dir() -> Option<PathBuf> {
        std::env::var_os("SIMBRIDGE_TEST_SVD_DIR").map(PathBuf::from)
    }

    #[test]
    fn stm32f407_usart_sr() {
        let Some(d) = svd_test_dir() else { return };
        let dev = load(&d.join("STM32F407.svd")).unwrap();
        let p = dev.find("usart2", Some(0x4000_4400)).unwrap();
        assert_eq!(p.name, "USART2");
        let sr = p.register("SR").unwrap();
        let v = sr.decode(0xC0, false);
        let f = v["fields"].as_array().unwrap();
        let get = |n: &str| f.iter().find(|x| x["name"] == n).unwrap()["value"].as_u64().unwrap();
        assert_eq!(get("TXE"), 1);
        assert_eq!(get("TC"), 1);
        assert_eq!(get("RXNE"), 0);
        // word at 0x4 holds DR
        let w = p.decode_word(4, 0x6B, false);
        assert_eq!(w[0]["register"], "DR");
        // name differs from Renode, base address resolves it
        assert_eq!(dev.find("gpioPortA", Some(0x4002_0000)).unwrap().name, "GPIOA");
        assert!(dev.search("TXEIE", 10).unwrap()["matches"].as_array().unwrap().len() >= 1);
    }

    /// Every SVD in the test directory must parse (vendor quirks included).
    #[test]
    fn all_parse() {
        let Some(d) = svd_test_dir() else { return };
        let mut failed = Vec::new();
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("svd")) {
                match load(&p) {
                    Ok(dev) => assert!(!dev.peripherals.is_empty(), "{}", p.display()),
                    Err(e) => failed.push(format!("{}: {e:#}", p.display())),
                }
            }
        }
        assert!(failed.is_empty(), "{failed:#?}");
    }

    #[test]
    fn nordic_lowercase_access() {
        let x = "<access>read-writeonce</access><usage> Read-Write </usage>";
        assert_eq!(normalize(x), "<access>read-writeOnce</access><usage>read-write</usage>");
    }

    #[test]
    fn e310x_uart() {
        let Some(d) = svd_test_dir() else { return };
        let dev = load(&d.join("e310x.svd")).unwrap();
        let p = dev.find("uart0", Some(0x1001_3000)).unwrap();
        assert_eq!(p.name, "UART0");
        let names: Vec<_> = p.registers.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"txdata"), "{names:?}");
        let tx = p.register("txdata").unwrap();
        let v = tx.decode(0x8000_0000, false);
        let full = v["fields"].as_array().unwrap().iter().find(|x| x["name"] == "full").unwrap().clone();
        assert_eq!(full["value"], 1);
    }
}
