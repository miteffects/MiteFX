//! Keys as the user's keyboard layout names them, while Ctrl is held (Linux, X11).
//!
//! baseview names a key from the user's layout, except with Ctrl down: then it names it as on a
//! US keyboard. On a German (QWERTZ) keyboard, Ctrl + the key labelled Z then arrives as Ctrl+Y. Here
//! the key's position is looked up in the X server's current layout (and layout group) instead.

use std::cell::OnceCell;

use keyboard_types::Code;
use x11rb::connection::Connection as _;
use x11rb::protocol::xkb::{ConnectionExt as _, ID};
use x11rb::protocol::xproto::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

thread_local! {
    /// The UI thread's own connection (opened at the first Ctrl shortcut) and whether XKB answers.
    static CONN: OnceCell<Option<(RustConnection, bool)>> = const { OnceCell::new() };
}

/// The evdev scan code of the letter and digit keys (X keycode = evdev + 8).
fn evdev(code: Code) -> Option<u8> {
    use Code::*;
    Some(match code {
        Digit1 => 2,
        Digit2 => 3,
        Digit3 => 4,
        Digit4 => 5,
        Digit5 => 6,
        Digit6 => 7,
        Digit7 => 8,
        Digit8 => 9,
        Digit9 => 10,
        Digit0 => 11,
        KeyQ => 16,
        KeyW => 17,
        KeyE => 18,
        KeyR => 19,
        KeyT => 20,
        KeyY => 21,
        KeyU => 22,
        KeyI => 23,
        KeyO => 24,
        KeyP => 25,
        KeyA => 30,
        KeyS => 31,
        KeyD => 32,
        KeyF => 33,
        KeyG => 34,
        KeyH => 35,
        KeyJ => 36,
        KeyK => 37,
        KeyL => 38,
        KeyZ => 44,
        KeyX => 45,
        KeyC => 46,
        KeyV => 47,
        KeyB => 48,
        KeyN => 49,
        KeyM => 50,
        _ => return None,
    })
}

/// A keysym's character, for the printable ones.
pub(crate) fn keysym_char(sym: u32) -> Option<char> {
    match sym {
        0x20..=0x7e | 0xa0..=0xff => char::from_u32(sym),
        0x0100_0100..=0x0110_ffff => char::from_u32(sym - 0x0100_0000),
        _ => None,
    }
}

/// The character the user's layout puts on the key at `code` (unshifted), or `None` for keys it
/// does not cover or when the X server cannot be asked.
pub fn layout_char(code: Code) -> Option<char> {
    let keycode = evdev(code)? + 8;
    CONN.with(|cell| {
        let (conn, xkb) = cell
            .get_or_init(|| {
                let (conn, _) = x11rb::connect(None).ok()?;
                let xkb = conn.xkb_use_extension(1, 0).ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.supported);
                Some((conn, xkb))
            })
            .as_ref()?;
        let group = if *xkb {
            conn.xkb_get_state(ID::USE_CORE_KBD.into()).ok().and_then(|c| c.reply().ok()).map_or(0, |s| usize::from(u8::from(s.group)))
        } else {
            0
        };
        let map = conn.get_keyboard_mapping(keycode, 1).ok()?.reply().ok()?;
        let per = usize::from(map.keysyms_per_keycode);
        let sym = |g: usize| map.keysyms.get(g * 2).copied().filter(|&s| s != 0 && g * 2 < per);
        let c = keysym_char(sym(group).or_else(|| sym(0))?)?;
        let _ = conn.flush();
        Some(c.to_lowercase().next().unwrap_or(c))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keysyms_name_their_characters() {
        assert_eq!(keysym_char(0x7a), Some('z'));
        assert_eq!(keysym_char(0xfc), Some('ü'));
        assert_eq!(keysym_char(0x0100_0161), Some('š'));
        assert_eq!(keysym_char(0xff0d), None, "Return is not a character");
        assert_eq!(evdev(Code::KeyY), Some(21));
    }
}

#[cfg(test)]
mod live {
    /// Prints what this machine's layout puts on the Y and Z keys (`--ignored --nocapture`).
    #[test]
    #[ignore = "needs an X server"]
    fn the_layout_here() {
        for c in [keyboard_types::Code::KeyY, keyboard_types::Code::KeyZ, keyboard_types::Code::KeyQ] {
            println!("{c:?} -> {:?}", super::layout_char(c));
        }
    }
}
