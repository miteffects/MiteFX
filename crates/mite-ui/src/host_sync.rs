//! Keeping GTK hosts' frame sync moving (Linux/X11).
//!
//! GNOME (Mutter) shows new content of an X11 window that uses extended frame sync
//! (`_NET_WM_SYNC_REQUEST_COUNTER`, set by GTK — e.g. Reaper's windows) when that window reports a
//! finished frame. An embedded plugin editor that animates while the host itself is idle never
//! makes the host report one: measured in Reaper, the FX window reported 0 frames/s while the
//! editor rendered 60, and the editor looked laggy only inside Reaper.
//!
//! After each presented frame we send the nearest frame-synced ancestor a 1×1 expose at its
//! corner. GTK repaints that pixel and reports a frame, so the compositor picks up our content
//! immediately. Hosts without frame sync, and standalone windows, are left alone.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, EventMask, ExposeEvent, EXPOSE_EVENT};
use x11rb::rust_connection::RustConnection;

pub(crate) struct HostSync {
    conn: RustConnection,
    target: u32,
}

impl HostSync {
    /// Finds the nearest ancestor of `window` that uses frame sync. `None` if there is none (or
    /// this is not an X11 window), in which case nothing needs nudging.
    pub fn find(window: &impl HasWindowHandle) -> Option<Self> {
        let raw = window.window_handle().ok()?.as_raw();
        if crate::window::tracing() {
            eprintln!("[mite-ui] host sync: own handle {raw:?}");
        }
        let id = match raw {
            RawWindowHandle::Xcb(h) => h.window.get(),
            RawWindowHandle::Xlib(h) => u32::try_from(h.window).ok()?,
            _ => return None,
        };
        let trace = crate::window::tracing();
        let (conn, _) = match x11rb::connect(None) {
            Ok(c) => c,
            Err(e) => {
                if trace {
                    eprintln!("[mite-ui] host sync: connect failed: {e}");
                }
                return None;
            }
        };
        let counter = conn.intern_atom(false, b"_NET_WM_SYNC_REQUEST_COUNTER").ok()?.reply().ok()?.atom;
        let mut current = id;
        for _ in 0..16 {
            let tree = conn.query_tree(current).ok()?.reply().ok()?;
            if trace {
                eprintln!("[mite-ui] host sync: {current:#x} -> parent {:#x} (root {:#x})", tree.parent, tree.root);
            }
            if tree.parent == tree.root || tree.parent == 0 {
                return None;
            }
            current = tree.parent;
            let prop = conn.get_property(false, current, counter, AtomEnum::CARDINAL, 0, 2).ok()?.reply().ok()?;
            if prop.value_len > 0 {
                return Some(Self { conn, target: current });
            }
        }
        None
    }

    /// Asks the host window to repaint one corner pixel, which makes it report a frame.
    pub fn nudge(&self) -> bool {
        let event = ExposeEvent {
            response_type: EXPOSE_EVENT,
            sequence: 0,
            window: self.target,
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            count: 0,
        };
        self.conn.send_event(false, self.target, EventMask::EXPOSURE, event).is_ok() && self.conn.flush().is_ok()
    }
}
