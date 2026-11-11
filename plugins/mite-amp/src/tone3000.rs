//! TONE3000 (https://www.tone3000.com): sign in, list the user's tones (favourites, downloads,
//! their own) and the site's trending and latest, see a tone's models, download one into a slot.
//!
//! Sign-in is OAuth's device flow (what TONE3000 documents for plugin hosts): the plugin shows a
//! code, the user enters it at tone3000.com/activate, the plugin polls until it is approved. The
//! session (refresh token) is kept in the settings folder. Every request runs on a worker
//! thread; the editor only reads [`Session::view`].
//!
//! The publishable key (`client_id`) is MiteFX's, built in; a build or the settings folder's
//! `tone3000.json` (`{"client_id": "…"}`) can override it. Downloads land in the settings
//! folder's `TONE3000/<tone>/` with the tone's creator and license kept for attribution.

use std::collections::{HashMap, VecDeque};
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::rig::{Rig, Slot, SlotFile, Source as Source3k};

pub const SITE: &str = "https://www.tone3000.com";
const API: &str = "https://www.tone3000.com/api/v1";
const UA: &str = concat!("MiteAmp/", env!("CARGO_PKG_VERSION"));

/// The plugin's settings folder (`~/.config/MiteFX/Mite Amp`, …).
pub fn settings_dir() -> Option<PathBuf> {
    mite_core::presets::user_dir(crate::presets::PLUGIN).and_then(|p| p.parent().map(PathBuf::from))
}

/// MiteFX's publishable key (TONE3000: safe to ship in client code; the secret key never is).
const MITEFX_CLIENT_ID: &str = "t3k_pub_pcHUFdaR-YOb1pUgOltWCWuTEUD7MZcQ";

/// The key: a build's own (`MITEFX_TONE3000_CLIENT_ID`), the settings folder's `tone3000.json`,
/// or MiteFX's.
fn client_id() -> Option<String> {
    if let Some(id) = option_env!("MITEFX_TONE3000_CLIENT_ID").filter(|s| !s.is_empty()) {
        return Some(id.to_owned());
    }
    let from_file = settings_dir().and_then(|d| std::fs::read_to_string(d.join("tone3000.json")).ok()).and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v.get("client_id").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned));
    Some(from_file.unwrap_or_else(|| MITEFX_CLIENT_ID.to_owned()))
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct Token {
    access: String,
    refresh: String,
    /// Seconds since the epoch.
    expires: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn token_path() -> Option<PathBuf> {
    Some(settings_dir()?.join("tone3000-session.json"))
}

fn load_token() -> Option<Token> {
    serde_json::from_str(&std::fs::read_to_string(token_path()?).ok()?).ok()
}

fn save_token(t: Option<&Token>) {
    let Some(path) = token_path() else { return };
    match t {
        Some(t) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(json) = serde_json::to_string(t) {
                let _ = std::fs::write(&path, json);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Where the tones shown come from: the search (with its filters) or one of the user's lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Source {
    #[default]
    Search,
    Favorites,
    Downloaded,
    Created,
}

impl Source {
    pub const ALL: [Source; 4] = [Source::Search, Source::Favorites, Source::Downloaded, Source::Created];

    pub fn name(self) -> &'static str {
        match self {
            Source::Search => "Explore",
            Source::Favorites => "Favorites",
            Source::Downloaded => "Downloaded",
            Source::Created => "My Tones",
        }
    }
}

/// The search's gear filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Gear {
    #[default]
    All,
    Amp,
    Pedal,
    FullRig,
    Cab,
    Outboard,
}

impl Gear {
    pub const ALL: [Gear; 6] = [Gear::All, Gear::Amp, Gear::Pedal, Gear::FullRig, Gear::Cab, Gear::Outboard];

    pub fn name(self) -> &'static str {
        match self {
            Gear::All => "All gear",
            Gear::Amp => "Amps",
            Gear::Pedal => "Pedals",
            Gear::FullRig => "Full rigs",
            Gear::Cab => "Cabs (IRs)",
            Gear::Outboard => "Outboard",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Gear::All => "",
            Gear::Amp => "&gears=amp",
            Gear::Pedal => "&gears=pedal",
            Gear::FullRig => "&gears=amp-cab",
            Gear::Cab => "&format=ir",
            Gear::Outboard => "&gears=outboard",
        }
    }
}

/// The search's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Trending,
    Downloads,
    Newest,
    BestMatch,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Trending, Sort::Downloads, Sort::Newest, Sort::BestMatch];

    pub fn name(self) -> &'static str {
        match self {
            Sort::Trending => "Trending",
            Sort::Downloads => "Most downloaded",
            Sort::Newest => "Newest",
            Sort::BestMatch => "Best match",
        }
    }

    fn query(self) -> &'static str {
        match self {
            Sort::Trending => "trending",
            Sort::Downloads => "downloads-all-time",
            Sort::Newest => "newest",
            Sort::BestMatch => "best-match",
        }
    }
}

/// What the search asks for.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Query {
    pub text: String,
    pub gear: Gear,
    pub sort: Sort,
    /// A1 captures instead of A2 (the site lists one generation at a time).
    pub a1: bool,
}

/// Tones per page.
const PAGE: u64 = 24;

fn url_encode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

impl Query {
    fn path(&self, source: Source, page: u64) -> String {
        match source {
            Source::Search => {
                let text = self.text.trim();
                let sort = if text.is_empty() && self.sort == Sort::BestMatch { Sort::Trending } else { self.sort };
                let q = if text.is_empty() { String::new() } else { format!("&query={}", url_encode(text)) };
                let arch = if self.gear == Gear::Cab { "" } else if self.a1 { "&architecture=1" } else { "&architecture=2" };
                format!("/tones/search?page={page}&page_size={PAGE}&sort={}{q}{}{arch}", sort.query(), self.gear.query())
            }
            Source::Favorites => format!("/tones/favorited?page={page}&page_size={PAGE}"),
            Source::Downloaded => format!("/tones/downloaded?page={page}&page_size={PAGE}"),
            Source::Created => format!("/tones/created?page={page}&page_size={PAGE}"),
        }
    }
}

/// A tone as listed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tone {
    pub id: u64,
    pub title: String,
    pub creator: String,
    pub description: String,
    pub gear: String,
    pub makes: Vec<String>,
    pub tags: Vec<String>,
    pub format: String,
    pub license: String,
    pub url: String,
    pub image: Option<String>,
    pub models: u64,
    pub a1_models: u64,
    pub a2_models: u64,
    pub irs: u64,
    pub downloads: u64,
    pub favorites: u64,
    pub is_favorite: bool,
}

impl Tone {
    /// "Amp", "Pedal", "Full rig", "Cab"…: what it is, in words.
    pub fn gear_name(&self) -> String {
        match self.gear.as_str() {
            "amp" => "Amp".to_owned(),
            "pedal" => "Pedal".to_owned(),
            "amp-cab" | "full-rig" => "Full rig".to_owned(),
            "cab" | "ir" => "Cab".to_owned(),
            "outboard" => "Outboard".to_owned(),
            "" if self.format == "ir" => "Cab".to_owned(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        }
    }
}

/// One of a tone's files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Model {
    pub id: u64,
    pub name: String,
    pub size: String,
    /// "A1", "A2", "IR" or the site's value.
    pub kind: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Phase {
    /// No publishable key: TONE3000 needs one (see the module docs).
    #[default]
    NoKey,
    SignedOut,
    Starting,
    /// Waiting for the user to enter `code` at `uri`.
    Code { code: String, uri: String },
    SignedIn,
    Failed(String),
}

/// The tones shown, page by page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Feed {
    pub source: Source,
    pub query: Query,
    pub tones: Vec<Tone>,
    pub page: u64,
    pub total: u64,
    pub total_pages: u64,
    pub loading: bool,
    pub error: Option<String>,
    /// Moves with every new search (late answers to an old one are dropped).
    generation: u64,
}

impl Feed {
    pub fn more(&self) -> bool {
        !self.loading && self.error.is_none() && self.page < self.total_pages
    }
}

/// Everything the editor draws, cloned out under the lock.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct View {
    pub phase: Phase,
    pub user: Option<String>,
    pub feed: Feed,
    /// The tone open, its files (none yet: loading), and an error.
    pub tone: Option<Tone>,
    pub models: Option<Vec<Model>>,
    pub models_error: Option<String>,
    /// A download under way (the model id) and the last one's error.
    pub downloading: Option<u64>,
    pub download_error: Option<String>,
}

/// A decoded cover image, scaled down for cards.
#[derive(Debug)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    /// RGBA8.
    pub rgba: Arc<Vec<u8>>,
}

#[derive(Debug, Clone)]
enum ImageState {
    Queued,
    Ready(Arc<Thumb>),
    Failed,
}

/// Cover images: fetched by a few workers, decoded (JPEG, PNG), scaled to at most 480 px wide.
#[derive(Default)]
struct Images {
    states: HashMap<String, ImageState>,
    queue: VecDeque<String>,
    workers: usize,
}

const IMAGE_WORKERS: usize = 4;
const THUMB_WIDTH: u32 = 480;

fn decode_image(bytes: &[u8]) -> Option<Thumb> {
    let (w, h, rgba) = if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
        decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut reader = decoder.read_info().ok()?;
        let mut buf = vec![0; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        let px = &buf[..info.buffer_size()];
        let rgba: Vec<u8> = match info.color_type {
            png::ColorType::Rgba => px.to_vec(),
            png::ColorType::Rgb => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            png::ColorType::GrayscaleAlpha => px.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
            png::ColorType::Indexed => return None,
        };
        (info.width, info.height, rgba)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
        let px = decoder.decode().ok()?;
        let info = decoder.info()?;
        let rgba: Vec<u8> = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => px.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
            jpeg_decoder::PixelFormat::L8 => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
            jpeg_decoder::PixelFormat::CMYK32 => px.chunks_exact(4).flat_map(|p| {
                let k = 255 - u16::from(p[3]);
                let c = |v: u8| ((255 - u16::from(v)) * k / 255) as u8;
                [c(p[0]), c(p[1]), c(p[2]), 255]
            }).collect(),
            _ => return None,
        };
        (u32::from(info.width), u32::from(info.height), rgba)
    } else {
        return None;
    };
    if w == 0 || h == 0 || rgba.len() < (w * h * 4) as usize {
        return None;
    }
    // Box-filter down to the card size.
    let step = w.div_ceil(THUMB_WIDTH).max(1);
    let (tw, th) = (w / step, h / step);
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        for x in 0..tw {
            let mut acc = [0u32; 4];
            for dy in 0..step {
                for dx in 0..step {
                    let i = (((y * step + dy) * w + x * step + dx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += u32::from(rgba[i + c]);
                    }
                }
            }
            let o = ((y * tw + x) * 4) as usize;
            for c in 0..4 {
                out[o + c] = (acc[c] / (step * step)) as u8;
            }
        }
    }
    Some(Thumb { width: tw, height: th, rgba: Arc::new(out) })
}

/// Where a tone's file is (or would be) kept.
pub fn local_path(tone: &Tone, model: &Model) -> Option<PathBuf> {
    let dir = settings_dir()?.join("TONE3000").join(format!("{}-{}", tone.id, safe_name(&tone.title)));
    let ext = if model.kind == "IR" { "wav" } else { "nam" };
    let url_name = model.url.split('?').next().unwrap_or_default().rsplit('/').next().unwrap_or_default();
    let name = if url_name.contains('.') { safe_name(url_name) } else { format!("{}.{ext}", safe_name(&model.name)) };
    Some(dir.join(name))
}

#[derive(Default)]
struct State {
    view: View,
    token: Option<Token>,
    client: Option<String>,
    /// Bumped by sign-out and sign-in so a stale poll stops.
    epoch: u64,
}

pub struct Session {
    state: Mutex<State>,
    images: Mutex<Images>,
    version: AtomicU64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
enum HttpError {
    /// The server's answer with its status and body.
    Status(u16, String),
    Other(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Status(code, body) => {
                let msg = serde_json::from_str::<Value>(body).ok().and_then(|v| v.get("error_description").or(v.get("message")).or(v.get("error")).and_then(Value::as_str).map(str::to_owned));
                write!(f, "TONE3000 answered {code}{}", msg.map(|m| format!(": {m}")).unwrap_or_default())
            }
            HttpError::Other(e) => write!(f, "{e}"),
        }
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(Duration::from_secs(60)).user_agent(UA).build()
}

fn answer(r: Result<ureq::Response, ureq::Error>) -> Result<ureq::Response, HttpError> {
    match r {
        Ok(resp) => Ok(resp),
        Err(ureq::Error::Status(code, resp)) => Err(HttpError::Status(code, resp.into_string().unwrap_or_default())),
        Err(e) => Err(HttpError::Other(format!("cannot reach TONE3000 ({e})"))),
    }
}

fn form(url: &str, fields: &[(&str, &str)]) -> Result<Value, HttpError> {
    let resp = answer(agent().post(url).send_form(fields))?;
    let text = resp.into_string().map_err(|e| HttpError::Other(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| HttpError::Other(format!("an unreadable answer ({e})")))
}

fn text_of(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn uint_of(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0)
}

fn parse_tone(v: &Value) -> Tone {
    let user = v.get("user").cloned().unwrap_or(Value::Null);
    let creator = Some(text_of(&user, "display_name")).filter(|s| !s.is_empty()).unwrap_or_else(|| text_of(&user, "username"));
    let gear = match v.get("gear") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let names = |k: &str| -> Vec<String> { v.get(k).and_then(Value::as_array).map(|a| a.iter().map(|m| text_of(m, "name")).filter(|n| !n.is_empty()).collect()).unwrap_or_default() };
    let image = v.get("images").and_then(Value::as_array).and_then(|a| a.iter().filter_map(Value::as_str).find(|u| u.starts_with("https://"))).map(str::to_owned);
    Tone {
        id: uint_of(v, "id"),
        title: text_of(v, "title"),
        creator,
        description: text_of(v, "description"),
        gear,
        makes: names("makes"),
        tags: names("tags"),
        format: text_of(v, "format"),
        license: text_of(v, "license"),
        url: text_of(v, "url"),
        image,
        models: uint_of(v, "models_count"),
        a1_models: uint_of(v, "a1_models_count"),
        a2_models: uint_of(v, "a2_models_count"),
        irs: uint_of(v, "irs_count"),
        downloads: uint_of(v, "downloads_count"),
        favorites: uint_of(v, "favorites_count"),
        is_favorite: v.get("is_favorite").and_then(Value::as_bool).unwrap_or(false),
    }
}

fn parse_model(v: &Value) -> Model {
    let kind = match v.get("architecture_version") {
        Some(Value::Null) | None => "IR".to_owned(),
        Some(Value::Number(n)) => format!("A{n}"),
        Some(Value::String(s)) if s.chars().all(|c| c.is_ascii_digit()) => format!("A{s}"),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    };
    Model { id: uint_of(v, "id"), name: text_of(v, "name"), size: text_of(v, "size"), kind, url: text_of(v, "model_url") }
}

/// A file name that is safe on every OS.
fn safe_name(s: &str) -> String {
    let cleaned: String = s.chars().map(|c| if c.is_alphanumeric() || " ._-()".contains(c) { c } else { '_' }).collect();
    let t = cleaned.trim().trim_matches('.').to_owned();
    if t.is_empty() { "model".to_owned() } else { t.chars().take(120).collect() }
}

impl Session {
    pub fn new() -> Self {
        let client = client_id();
        let token = load_token();
        let phase = match (&client, &token) {
            (None, _) => Phase::NoKey,
            (Some(_), Some(_)) => Phase::SignedIn,
            (Some(_), None) => Phase::SignedOut,
        };
        let state = State { view: View { phase, ..View::default() }, token, client, epoch: 0 };
        Self { state: Mutex::new(state), images: Mutex::new(Images::default()), version: AtomicU64::new(0) }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn changed(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
    }

    /// Moves on every change (the editor redraws for it).
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn view(&self) -> View {
        self.lock().view.clone()
    }

    fn update(&self, f: impl FnOnce(&mut View)) {
        f(&mut self.lock().view);
        self.changed();
    }

    /// Starts the device sign-in.
    pub fn sign_in(self: &Arc<Self>) {
        let (client, epoch) = {
            let mut st = self.lock();
            let Some(client) = st.client.clone() else { return };
            st.epoch += 1;
            st.view.phase = Phase::Starting;
            (client, st.epoch)
        };
        self.changed();
        let me = self.clone();
        std::thread::spawn(move || me.device_flow(&client, epoch));
    }

    fn device_flow(&self, client: &str, epoch: u64) {
        let fail = |e: String| self.update(|v| v.phase = Phase::Failed(e));
        let start = match form(&format!("{API}/oauth/device_authorization"), &[("client_id", client)]) {
            Ok(v) => v,
            Err(e) => return fail(e.to_string()),
        };
        let device = text_of(&start, "device_code");
        let code = text_of(&start, "user_code");
        let uri = Some(text_of(&start, "verification_uri_complete")).filter(|s| !s.is_empty()).unwrap_or_else(|| text_of(&start, "verification_uri"));
        let mut interval = uint_of(&start, "interval").max(5);
        let expires = Instant::now() + Duration::from_secs(uint_of(&start, "expires_in").max(60));
        if device.is_empty() || code.is_empty() {
            return fail("TONE3000 did not give a sign-in code".to_owned());
        }
        self.update(|v| v.phase = Phase::Code { code, uri: uri.clone() });
        open_in_browser(&uri);
        while Instant::now() < expires {
            std::thread::sleep(Duration::from_secs(interval));
            if self.lock().epoch != epoch {
                return;
            }
            let r = form(&format!("{API}/oauth/token"), &[("grant_type", "urn:ietf:params:oauth:grant-type:device_code"), ("device_code", &device), ("client_id", client)]);
            match r {
                Ok(v) => {
                    let token = Token { access: text_of(&v, "access_token"), refresh: text_of(&v, "refresh_token"), expires: now() + uint_of(&v, "expires_in").max(60) };
                    save_token(Some(&token));
                    {
                        let mut st = self.lock();
                        st.token = Some(token);
                        st.view.phase = Phase::SignedIn;
                    }
                    self.changed();
                    self.fetch_user();
                    return;
                }
                Err(HttpError::Status(_, body)) => {
                    let err = serde_json::from_str::<Value>(&body).ok().map(|v| text_of(&v, "error")).unwrap_or_default();
                    match err.as_str() {
                        "authorization_pending" => {}
                        "slow_down" => interval += 5,
                        "access_denied" => return fail("Sign-in was declined".to_owned()),
                        "expired_token" => return fail("The code expired: sign in again".to_owned()),
                        _ => return fail(HttpError::Status(400, body).to_string()),
                    }
                }
                Err(e) => return fail(e.to_string()),
            }
        }
        fail("The code expired: sign in again".to_owned());
    }

    pub fn sign_out(&self) {
        save_token(None);
        {
            let mut st = self.lock();
            st.epoch += 1;
            st.token = None;
            let phase = if st.client.is_some() { Phase::SignedOut } else { Phase::NoKey };
            st.view = View { phase, ..View::default() };
        }
        self.changed();
    }

    /// A valid access token (refreshed when it has run out).
    fn access(&self) -> Result<String, String> {
        let (token, client) = {
            let st = self.lock();
            (st.token.clone(), st.client.clone())
        };
        let token = token.ok_or("Not signed in")?;
        if token.expires > now() + 30 {
            return Ok(token.access);
        }
        let client = client.ok_or("No TONE3000 key")?;
        match form(&format!("{API}/oauth/token"), &[("grant_type", "refresh_token"), ("refresh_token", &token.refresh), ("client_id", &client)]) {
            Ok(v) => {
                let refresh = Some(text_of(&v, "refresh_token")).filter(|s| !s.is_empty()).unwrap_or(token.refresh);
                let fresh = Token { access: text_of(&v, "access_token"), refresh, expires: now() + uint_of(&v, "expires_in").max(60) };
                save_token(Some(&fresh));
                self.lock().token = Some(fresh.clone());
                Ok(fresh.access)
            }
            Err(HttpError::Status(400, _)) | Err(HttpError::Status(401, _)) => {
                self.sign_out();
                Err("The TONE3000 session ended: sign in again".to_owned())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn get(&self, path: &str) -> Result<Value, String> {
        let token = self.access()?;
        let resp = answer(agent().get(&format!("{API}{path}")).set("Authorization", &format!("Bearer {token}")).call()).map_err(|e| e.to_string())?;
        let text = resp.into_string().map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| format!("an unreadable answer ({e})"))
    }

    fn fetch_user(&self) {
        if let Ok(u) = self.get("/user") {
            let name = Some(text_of(&u, "display_name")).filter(|s| !s.is_empty()).unwrap_or_else(|| text_of(&u, "username"));
            self.update(|v| v.user = Some(name).filter(|n| !n.is_empty()));
        }
    }

    /// Shows `source` with `query` (a new search: from the first page).
    pub fn search(self: &Arc<Self>, source: Source, query: Query) {
        let generation = {
            let mut st = self.lock();
            let g = st.view.feed.generation + 1;
            st.view.feed = Feed { source, query, loading: true, generation: g, ..Feed::default() };
            g
        };
        self.changed();
        self.fetch_page(generation, 1);
    }

    /// The next page of what is shown.
    pub fn more(self: &Arc<Self>) {
        let (generation, page) = {
            let mut st = self.lock();
            if !st.view.feed.more() {
                return;
            }
            st.view.feed.loading = true;
            (st.view.feed.generation, st.view.feed.page + 1)
        };
        self.changed();
        self.fetch_page(generation, page);
    }

    fn fetch_page(self: &Arc<Self>, generation: u64, page: u64) {
        let (source, query) = {
            let st = self.lock();
            (st.view.feed.source, st.view.feed.query.clone())
        };
        let me = self.clone();
        std::thread::spawn(move || {
            let result = me.get(&query.path(source, page));
            if me.lock().view.user.is_none() {
                me.fetch_user();
            }
            me.update(|v| {
                let feed = &mut v.feed;
                if feed.generation != generation {
                    return;
                }
                feed.loading = false;
                match result {
                    Ok(body) => {
                        let tones: Vec<Tone> = body.get("data").and_then(Value::as_array).map(|a| a.iter().map(parse_tone).collect()).unwrap_or_default();
                        for t in tones {
                            if !feed.tones.iter().any(|x| x.id == t.id) {
                                feed.tones.push(t);
                            }
                        }
                        feed.page = page;
                        feed.total = uint_of(&body, "total").max(feed.tones.len() as u64);
                        feed.total_pages = uint_of(&body, "total_pages").max(page);
                    }
                    Err(e) => feed.error = Some(e),
                }
            });
        });
    }

    /// Opens a tone: fetches its files (A2 first, then A1; or its IRs).
    pub fn open_tone(self: &Arc<Self>, tone: Tone) {
        self.update(|v| {
            v.tone = Some(tone.clone());
            v.models = None;
            v.models_error = None;
            v.download_error = None;
        });
        let me = self.clone();
        std::thread::spawn(move || {
            let fetch = |arch: Option<&str>| -> Result<Vec<Model>, String> {
                let q = arch.map(|a| format!("&architecture={a}")).unwrap_or_default();
                let v = me.get(&format!("/models?tone_id={}&page=1&page_size=300{q}", tone.id))?;
                Ok(v.get("data").and_then(Value::as_array).map(|a| a.iter().map(parse_model).collect()).unwrap_or_default())
            };
            let result = if tone.format == "ir" {
                fetch(None)
            } else {
                // Without an architecture the site leaves A2 out: ask for A2, then the rest.
                fetch(Some("2")).and_then(|mut all| {
                    for m in fetch(None)? {
                        if !all.iter().any(|x| x.id == m.id) {
                            all.push(m);
                        }
                    }
                    Ok(all)
                })
            };
            me.update(|v| {
                if v.tone.as_ref().map(|t| t.id) == Some(tone.id) {
                    match result {
                        Ok(m) => v.models = Some(m),
                        Err(e) => {
                            v.models = Some(Vec::new());
                            v.models_error = Some(e);
                        }
                    }
                }
            });
        });
    }

    /// Back from a tone to the cards.
    pub fn close_tone(&self) {
        self.update(|v| {
            v.tone = None;
            v.models = None;
        });
    }

    /// Favourites a tone on the site, or takes it back.
    pub fn set_favorite(self: &Arc<Self>, tone_id: u64, on: bool) {
        let flip = move |v: &mut View, on: bool| {
            let delta = move |t: &mut Tone| {
                if t.id == tone_id && t.is_favorite != on {
                    t.is_favorite = on;
                    t.favorites = if on { t.favorites + 1 } else { t.favorites.saturating_sub(1) };
                }
            };
            v.feed.tones.iter_mut().for_each(delta);
            if let Some(t) = &mut v.tone {
                delta(t);
            }
        };
        self.update(|v| flip(v, on));
        let me = self.clone();
        std::thread::spawn(move || {
            let result = me.access().and_then(|token| {
                let url = format!("{API}/tones/{tone_id}/favorite");
                let req = if on { agent().put(&url) } else { agent().delete(&url) };
                answer(req.set("Authorization", &format!("Bearer {token}")).call()).map(|_| ()).map_err(|e| e.to_string())
            });
            if result.is_err() {
                me.update(|v| flip(v, !on));
            }
        });
    }

    /// The cover image at `url`, once fetched (asks for it the first time).
    pub fn image(self: &Arc<Self>, url: &str) -> Option<Arc<Thumb>> {
        let mut im = self.images.lock().unwrap_or_else(|e| e.into_inner());
        match im.states.get(url) {
            Some(ImageState::Ready(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        im.states.insert(url.to_owned(), ImageState::Queued);
        im.queue.push_back(url.to_owned());
        if im.workers < IMAGE_WORKERS {
            im.workers += 1;
            let me = self.clone();
            std::thread::spawn(move || me.image_worker());
        }
        None
    }

    fn image_worker(&self) {
        loop {
            let next = {
                let mut im = self.images.lock().unwrap_or_else(|e| e.into_inner());
                match im.queue.pop_back() {
                    Some(u) => u,
                    None => {
                        im.workers -= 1;
                        return;
                    }
                }
            };
            let thumb = answer(agent().get(&next).call()).ok().and_then(|resp| {
                let mut bytes = Vec::new();
                resp.into_reader().take(16 << 20).read_to_end(&mut bytes).ok()?;
                decode_image(&bytes)
            });
            let state = match thumb {
                Some(t) => ImageState::Ready(Arc::new(t)),
                None => ImageState::Failed,
            };
            self.images.lock().unwrap_or_else(|e| e.into_inner()).states.insert(next, state);
            self.changed();
        }
    }

    /// Downloads `model` of `tone` (or takes the copy on disk) and puts it in `slot`.
    pub fn download(self: &Arc<Self>, tone: Tone, model: Model, slot: Slot, rig: Arc<Rig>) {
        self.update(|v| {
            v.downloading = Some(model.id);
            v.download_error = None;
        });
        let me = self.clone();
        std::thread::spawn(move || {
            let result = me.fetch_file(&tone, &model);
            match result {
                Ok(path) => {
                    let source = Source3k { tone_id: tone.id, model_id: model.id, title: tone.title.clone(), creator: tone.creator.clone(), license: tone.license.clone(), url: tone.url.clone(), gear: if tone.format == "ir" { "ir".to_owned() } else { tone.gear.clone() } };
                    rig.set_file(slot, SlotFile { path: Some(path), source: Some(source) });
                    me.update(|v| v.downloading = None);
                }
                Err(e) => me.update(|v| {
                    v.downloading = None;
                    v.download_error = Some(e);
                }),
            }
        });
    }

    fn fetch_file(&self, tone: &Tone, model: &Model) -> Result<PathBuf, String> {
        if model.url.is_empty() {
            return Err("this file has no download address".to_owned());
        }
        let path = local_path(tone, model).ok_or("no settings folder")?;
        let dir = path.parent().map(PathBuf::from).ok_or("no settings folder")?;
        if path.is_file() {
            return Ok(path);
        }
        let token = self.access()?;
        let resp = answer(agent().get(&model.url).set("Authorization", &format!("Bearer {token}")).call()).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        resp.into_reader().take(256 << 20).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("part");
        std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        // Attribution travels with the file.
        let about = serde_json::json!({ "tone": tone.title, "creator": tone.creator, "license": tone.license, "url": tone.url, "model": model.name, "source": SITE });
        let _ = std::fs::write(dir.join("about-tone3000.json"), about.to_string());
        Ok(path)
    }
}

/// Opens `url` in the user's browser (best effort).
pub fn open_in_browser(url: &str) {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return;
    }
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_and_lists_ask_for_the_right_page() {
        let q = Query { text: "plexi 1959".into(), gear: Gear::Amp, sort: Sort::BestMatch, a1: false };
        assert_eq!(q.path(Source::Search, 3), "/tones/search?page=3&page_size=24&sort=best-match&query=plexi%201959&gears=amp&architecture=2");
        // No words: "best match" means nothing, trending instead; cabs are IRs, any generation.
        let q = Query { gear: Gear::Cab, sort: Sort::BestMatch, ..Query::default() };
        assert_eq!(q.path(Source::Search, 1), "/tones/search?page=1&page_size=24&sort=trending&format=ir");
        assert_eq!(Query::default().path(Source::Favorites, 2), "/tones/favorited?page=2&page_size=24");
        let feed = Feed { page: 1, total_pages: 3, ..Feed::default() };
        assert!(feed.more());
        assert!(!Feed { loading: true, ..feed.clone() }.more());
        assert!(!Feed { page: 3, ..feed }.more());
    }

    #[test]
    fn tones_and_models_read_from_the_apis_shapes() {
        let tone: Value = serde_json::from_str(r#"{"id": 27156, "title": "Plexi 1959", "user": {"username": "amp_guy", "display_name": null}, "gear": "amp", "format": "nam", "license": "cc-by", "url": "https://www.tone3000.com/tones/plexi-27156", "models_count": 4, "a2_models_count": 2, "irs_count": 0}"#).unwrap();
        let t = parse_tone(&tone);
        assert_eq!((t.id, t.title.as_str(), t.creator.as_str(), t.models, t.a2_models), (27156, "Plexi 1959", "amp_guy", 4, 2));
        let m: Value = serde_json::from_str(r#"{"id": 5, "name": "Crunch", "size": "standard", "architecture_version": 2, "model_url": "https://cdn.tone3000.com/m/crunch.nam?sig=1"}"#).unwrap();
        assert_eq!(parse_model(&m).kind, "A2");
        let ir: Value = serde_json::from_str(r#"{"id": 6, "name": "V30", "architecture_version": null, "model_url": "x"}"#).unwrap();
        assert_eq!(parse_model(&ir).kind, "IR");
        assert_eq!(safe_name("a/b:c?.nam"), "a_b_c_.nam");
        assert_eq!(safe_name(".."), "model");
    }
}
