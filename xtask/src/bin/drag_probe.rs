//! Diagnostics: drives an embedded editor with synthetic X11 mouse events (no real pointer
//! movement) and prints the wall-clock time of each step, so a screen capture of the window can
//! measure input-to-screen latency.
//!
//!     cargo run -p xtask --bin drag_probe -- "Mite EQ" X Y STEPS STEP_PX STEP_MS
//! X, Y: start point in the editor window's pixels. Adds a band there (double-click), then drags it.

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::*;

    let a: Vec<String> = std::env::args().skip(1).collect();
    let needle = &a[0];
    let (x0, y0): (i16, i16) = (a[1].parse()?, a[2].parse()?);
    let (steps, step_px, step_ms): (u32, i16, u64) = (a[3].parse()?, a[4].parse()?, a[5].parse()?);
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let clients = conn.intern_atom(false, b"_NET_CLIENT_LIST")?.reply()?.atom;
    let list = conn.get_property(false, root, clients, AtomEnum::WINDOW, 0, 4096)?.reply()?;
    let top = list
        .value32()
        .into_iter()
        .flatten()
        .find(|&w| {
            conn.get_property(false, w, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
                .ok()
                .and_then(|c| c.reply().ok())
                .is_some_and(|r| String::from_utf8_lossy(&r.value).contains(needle.as_str()))
        })
        .ok_or("window not found")?;
    // The editor is the largest descendant created by a different X client than the host (X
    // resource ids carry the client in their high bits).
    let client_bits = |w: u32| w & !0x1F_FFFF;
    let mut best = (0u32, 0u32);
    let mut stack = vec![top];
    while let Some(w) = stack.pop() {
        for child in conn.query_tree(w)?.reply()?.children {
            let g = conn.get_geometry(child)?.reply()?;
            let area = u32::from(g.width) * u32::from(g.height);
            if client_bits(child) != client_bits(top) && area > best.1 {
                best = (child, area);
            }
            stack.push(child);
        }
    }
    if best.0 == 0 {
        return Err("editor window not found".into());
    }
    eprintln!("editor window {:#x}", best.0);
    let target = best.0;
    let now = || SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    let send = |kind: u8, x: i16, y: i16, state: u16| -> Result<(), Box<dyn std::error::Error>> {
        let ev = ButtonPressEvent {
            response_type: kind,
            detail: if kind == MOTION_NOTIFY_EVENT { 0 } else { 1 },
            sequence: 0,
            time: 0,
            root,
            event: target,
            child: 0,
            root_x: x,
            root_y: y,
            event_x: x,
            event_y: y,
            state: state.into(),
            same_screen: true,
        };
        let mask = match kind {
            BUTTON_PRESS_EVENT => EventMask::BUTTON_PRESS,
            BUTTON_RELEASE_EVENT => EventMask::BUTTON_RELEASE,
            _ => EventMask::POINTER_MOTION | EventMask::BUTTON1_MOTION,
        };
        conn.send_event(false, target, mask, ev)?;
        conn.flush()?;
        Ok(())
    };
    // Add a band: move there, double-click.
    send(MOTION_NOTIFY_EVENT, x0, y0, 0)?;
    for _ in 0..2 {
        send(BUTTON_PRESS_EVENT, x0, y0, 0)?;
        send(BUTTON_RELEASE_EVENT, x0, y0, 0x100)?;
        std::thread::sleep(Duration::from_millis(60));
    }
    std::thread::sleep(Duration::from_millis(500));
    // Drag it right in steps.
    send(BUTTON_PRESS_EVENT, x0, y0, 0)?;
    std::thread::sleep(Duration::from_millis(200));
    for i in 1..=steps {
        let x = x0 + step_px * i as i16;
        let t = now();
        send(MOTION_NOTIFY_EVENT, x, y0, 0x100)?;
        println!("{t:.6}");
        std::thread::sleep(Duration::from_millis(step_ms));
    }
    send(BUTTON_RELEASE_EVENT, x0 + step_px * steps as i16, y0, 0x100)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn main() {}
