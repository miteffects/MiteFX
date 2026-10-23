//! Renders cases through plugins in a separate Reaper instance (a copy of the user's Reaper
//! resource folder, so its plugin scans are reused and its settings untouched).
//!
//! One generated Lua script runs every case: add a track, insert the plugin, set its parameters
//! by what the plugin displays (a bisection on the normalized value until the formatted value
//! reads the target, or a scan for a choice's text, so neither vendor's scaling needs knowing),
//! insert the signal, render, note the plugin's reported latency, its displayed settings and the
//! render time, and remove the track. Reaper's delay compensation is on, so a correctly reported
//! latency leaves the output aligned with the input.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// A parameter target: a number the formatted value should read, a choice's text, or (for our
/// own plugins, whose mapping we know) an exact normalized value at a parameter index.
#[derive(Debug, Clone)]
pub enum Value {
    Num(f64),
    Text(String),
    Index(usize, f64),
}

/// One render: `fx` (Reaper's name, e.g. `VST3:Pro-Q 4 (FabFilter)`) on `signal`, at `rate`.
#[derive(Debug, Clone)]
pub struct Case {
    pub id: String,
    pub fx: String,
    pub signal: PathBuf,
    pub rate: u32,
    pub params: Vec<(String, Value)>,
}

pub fn num(name: &str, v: f64) -> (String, Value) {
    (name.to_owned(), Value::Num(v))
}

pub fn text(name: &str, v: &str) -> (String, Value) {
    (name.to_owned(), Value::Text(v.to_owned()))
}

/// What a render left: the reported latency (samples), the render time (s) and the parameters as
/// the plugin displayed them.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub latency: i64,
    pub seconds: f64,
    pub settings: Vec<(String, String)>,
}

fn lua_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

const LUA_HEAD: &str = r#"
local log = io.open(LOG, "w")
local function parse(s)
  s = s:gsub("\226\136\146", "-")
  if s:find("INF") then return s:find("-INF") and -1e9 or 1e9 end
  if s:find("\226\136\158") then return 1e9 end
  local v = s:match("[-+]?%d+%.?%d*")
  if not v then return nil end
  v = tonumber(v)
  if s:find("kHz") then v = v * 1000 end
  -- Times read in ms: "1.000 sec", "1.20 s".
  if s:find("sec") or s:find("%d s$") then v = v * 1000 end
  return v
end
local function find(tr, fx, name)
  for i = 0, reaper.TrackFX_GetNumParams(tr, fx) - 1 do
    local _, pn = reaper.TrackFX_GetParamName(tr, fx, i, "")
    if pn == name then return i end
  end
  return nil
end
local function formatted(tr, fx, i)
  local _, f = reaper.TrackFX_GetFormattedParamValue(tr, fx, i, "")
  return f
end
local function set_num(tr, fx, i, target)
  local function at(n) reaper.TrackFX_SetParamNormalized(tr, fx, i, n); return parse(formatted(tr, fx, i)) or 0 end
  local up = at(1) >= at(0)
  local lo, hi = 0.0, 1.0
  for _ = 1, 48 do
    local mid = 0.5 * (lo + hi)
    if (at(mid) < target) == up then lo = mid else hi = mid end
  end
  local a, b = at(lo), at(hi)
  reaper.TrackFX_SetParamNormalized(tr, fx, i, (math.abs(a - target) <= math.abs(b - target)) and lo or hi)
end
local function set_text(tr, fx, i, target)
  local want = target:lower()
  for k = 0, 1000 do
    local n = k / 1000
    reaper.TrackFX_SetParamNormalized(tr, fx, i, n)
    if formatted(tr, fx, i):lower() == want then return true end
  end
  return false
end
"#;

/// The script for `cases`, rendering into `out`, logging to `log`, touching `done` at the end.
fn script(cases: &[Case], out: &Path, log: &Path, done: &Path) -> String {
    let mut s = format!("local LOG = {}\n{LUA_HEAD}", lua_str(&log.to_string_lossy()));
    let mut rate = 0;
    for c in cases {
        if c.rate != rate {
            rate = c.rate;
            let _ = writeln!(s, "reaper.GetSetProjectInfo(0, \"PROJECT_SRATE\", {rate}, true)\nreaper.GetSetProjectInfo(0, \"PROJECT_SRATE_USE\", 1, true)");
        }
        let _ = writeln!(s, "do\n  log:write(\"case \", {}, \"\\n\")", lua_str(&c.id));
        let _ = writeln!(
            s,
            "  reaper.InsertTrackAtIndex(0, false)\n  local tr = reaper.GetTrack(0, 0)\n  local fx = reaper.TrackFX_AddByName(tr, {}, false, -1)\n  if fx < 0 then log:write(\"error no fx\\n\") end",
            lua_str(&c.fx)
        );
        for (name, v) in &c.params {
            if let Value::Index(i, n) = v {
                let _ = writeln!(s, "  reaper.TrackFX_SetParamNormalized(tr, fx, {i}, {n})");
                continue;
            }
            let _ = writeln!(s, "  do local i = find(tr, fx, {n})\n    if not i then log:write(\"error no param \", {n}, \"\\n\") else", n = lua_str(name));
            match v {
                Value::Num(x) => {
                    let _ = writeln!(s, "      set_num(tr, fx, i, {x})");
                }
                Value::Text(t) => {
                    let _ = writeln!(s, "      if not set_text(tr, fx, i, {t}) then log:write(\"error no choice \", {n}, \" = \", {t}, \"\\n\") end", t = lua_str(t), n = lua_str(name));
                }
                Value::Index(..) => {}
            }
            let _ = writeln!(s, "      log:write(\"set \", {n}, \"\\t\", formatted(tr, fx, i), \"\\n\") end end", n = lua_str(name));
        }
        // Parameters set by index arrive with the next audio block (the plugin is processing):
        // wait, then log what the plugin shows.
        // Every case then settles for a second before rendering: background work a setting
        // starts (a linear-phase kernel being designed) finishes, as it would before playback.
        let indexed: Vec<usize> = c.params.iter().filter_map(|(_, v)| if let Value::Index(i, _) = v { Some(*i) } else { None }).collect();
        let _ = writeln!(s, "  local until_t = reaper.time_precise() + 1.0 while reaper.time_precise() < until_t do end");
        if !indexed.is_empty() {
            for i in indexed {
                let _ = writeln!(s, "  do local _, pn = reaper.TrackFX_GetParamName(tr, fx, {i}, \"\") log:write(\"set \", pn, \"\\t\", formatted(tr, fx, {i}), \"\\n\") end");
            }
        }
        let _ = writeln!(
            s,
            r#"  reaper.SetOnlyTrackSelected(tr)
  reaper.SetEditCurPos(0, false, false)
  reaper.InsertMedia({sig}, 0)
  reaper.GetSetProjectInfo_String(0, "RENDER_FILE", {dir}, true)
  reaper.GetSetProjectInfo_String(0, "RENDER_PATTERN", {id}, true)
  reaper.GetSetProjectInfo_String(0, "RENDER_FORMAT", "ZXZhdyAAAQ==", true)
  reaper.GetSetProjectInfo(0, "RENDER_SRATE", {rate}, true)
  reaper.GetSetProjectInfo(0, "RENDER_CHANNELS", 2, true)
  reaper.GetSetProjectInfo(0, "RENDER_BOUNDSFLAG", 1, true)
  reaper.GetSetProjectInfo(0, "RENDER_SETTINGS", 0, true)
  reaper.GetSetProjectInfo(0, "RENDER_DITHER", 0, true)
  local t0 = reaper.time_precise()
  reaper.Main_OnCommand(42230, 0)
  -- After the render: a plugin reports a changed latency once it has been re-activated.
  local _, pdc = reaper.TrackFX_GetNamedConfigParm(tr, fx, "pdc")
  log:write("latency ", tostring(pdc), "\n", "seconds ", reaper.time_precise() - t0, "\n")
  reaper.DeleteTrack(tr)
  log:flush()
end"#,
            sig = lua_str(&c.signal.to_string_lossy()),
            dir = lua_str(&out.to_string_lossy()),
            id = lua_str(&c.id),
            rate = c.rate,
        );
    }
    let _ = writeln!(s, "log:close()\nlocal f = io.open({}, \"w\") f:write(\"done\") f:close()", lua_str(&done.to_string_lossy()));
    s
}

/// Renders `cases` into `work/renders` and returns what each left, by case id.
pub fn render(cases: &[Case], work: &Path) -> Result<std::collections::BTreeMap<String, Rendered>, String> {
    let res = work.join("reaper-res");
    if !res.join("reaper.ini").exists() {
        let home = std::env::var("HOME").map_err(|e| e.to_string())?;
        copy_dir(&Path::new(&home).join(".config/REAPER"), &res)?;
    }
    let out = work.join("renders");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let (log, done, lua) = (work.join("render.log"), work.join("render.done"), work.join("render.lua"));
    let _ = std::fs::remove_file(&done);
    for c in cases {
        let _ = std::fs::remove_file(out.join(format!("{}.wav", c.id)));
    }
    std::fs::write(&lua, script(cases, &out, &log, &done)).map_err(|e| e.to_string())?;
    let mut child = Command::new("reaper")
        .args(["-newinst", "-nosplash", "-cfgfile"])
        .arg(res.join("reaper.ini"))
        .arg("-new")
        .arg(&lua)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("reaper: {e}"))?;
    let start = Instant::now();
    let limit = Duration::from_secs(60 + 20 * cases.len() as u64);
    while !done.exists() && start.elapsed() < limit && child.try_wait().ok().flatten().is_none() {
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = child.kill();
    let _ = child.wait();
    if !done.exists() {
        return Err(format!("Reaper did not finish within {} s; see {}", limit.as_secs(), log.display()));
    }
    parse_log(&std::fs::read_to_string(&log).map_err(|e| e.to_string())?)
}

/// What the last render left (`--analyze-only`).
pub fn load_log(work: &Path) -> Result<std::collections::BTreeMap<String, Rendered>, String> {
    let log = work.join("render.log");
    parse_log(&std::fs::read_to_string(&log).map_err(|e| format!("{}: {e}", log.display()))?)
}

fn parse_log(text: &str) -> Result<std::collections::BTreeMap<String, Rendered>, String> {
    let mut map = std::collections::BTreeMap::new();
    let mut current: Option<(String, Rendered)> = None;
    let mut errors = Vec::new();
    for line in text.lines() {
        if let Some(id) = line.strip_prefix("case ") {
            if let Some((k, v)) = current.take() {
                map.insert(k, v);
            }
            current = Some((id.to_owned(), Rendered::default()));
        } else if let Some((_, r)) = current.as_mut() {
            if let Some(v) = line.strip_prefix("latency ") {
                r.latency = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("seconds ") {
                r.seconds = v.trim().parse().unwrap_or(0.0);
            } else if let Some(v) = line.strip_prefix("set ") {
                let (k, val) = v.split_once('\t').unwrap_or((v, ""));
                r.settings.push((k.to_owned(), val.to_owned()));
            } else if line.starts_with("error") {
                errors.push(format!("{}: {line}", current.as_ref().map_or("", |c| c.0.as_str())));
            }
        }
    }
    if let Some((k, v)) = current {
        map.insert(k, v);
    }
    if !errors.is_empty() {
        return Err(format!("render errors:\n{}", errors.join("\n")));
    }
    Ok(map)
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let name = entry.file_name();
        if name == "Backups" {
            continue;
        }
        if path.is_dir() {
            copy_dir(&path, &to.join(&name))?;
        } else {
            std::fs::copy(&path, to.join(&name)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}
