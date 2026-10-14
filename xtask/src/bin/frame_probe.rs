//! Diagnostics: how often a GTK host window (e.g. Reaper's FX window) reports finished frames to
//! the compositor, via its extended _NET_WM_SYNC_REQUEST_COUNTER. GNOME/Mutter shows new content
//! of such windows at that rate, embedded plugin windows included.
//!
//!     cargo run -p xtask --bin frame_probe -- "Mite EQ" [seconds]

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use x11rb::connection::Connection;
    use x11rb::protocol::sync::ConnectionExt as _;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _};

    let mut args = std::env::args().skip(1);
    let needle = args.next().unwrap_or_else(|| "Mite EQ".into());
    let seconds: f64 = args.next().map_or(Ok(3.0), |s| s.parse())?;
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atom = |name: &str| -> Result<u32, Box<dyn std::error::Error>> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let (clients, counter_atom) = (atom("_NET_CLIENT_LIST")?, atom("_NET_WM_SYNC_REQUEST_COUNTER")?);
    let list = conn.get_property(false, root, clients, AtomEnum::WINDOW, 0, 4096)?.reply()?;
    let windows: Vec<u32> = list.value32().map(|v| v.collect()).unwrap_or_default();
    for w in windows {
        let name = conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)?.reply()?;
        let name = String::from_utf8_lossy(&name.value).to_string();
        if !name.contains(&needle) {
            continue;
        }
        if std::env::var_os("GEOMETRY").is_some() {
            let geo = conn.get_geometry(w)?.reply()?;
            let pos = conn.translate_coordinates(w, root, 0, 0)?.reply()?;
            println!("{} {} {} {}", pos.dst_x, pos.dst_y, geo.width, geo.height);
            return Ok(());
        }
        let prop = conn.get_property(false, w, counter_atom, AtomEnum::CARDINAL, 0, 2)?.reply()?;
        let counters: Vec<u32> = prop.value32().map(|v| v.collect()).unwrap_or_default();
        let Some(&counter) = counters.last() else {
            println!("{name}: no sync counter");
            continue;
        };
        let read = || -> Result<i64, Box<dyn std::error::Error>> {
            let v = conn.sync_query_counter(counter)?.reply()?.counter_value;
            Ok((i64::from(v.hi) << 32) | i64::from(v.lo))
        };
        let a = read()?;
        std::thread::sleep(std::time::Duration::from_secs_f64(seconds));
        let b = read()?;
        // Extended sync: odd = frame started, even = frame done; two increments per frame.
        println!("{name}: {:.1} frames/s reported to the compositor", (b - a) as f64 / 2.0 / seconds);
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {}
