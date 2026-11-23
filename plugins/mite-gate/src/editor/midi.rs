//! The MIDI trigger's panel (brief §1.5), docked over the history's bottom as the sidechain EQ's:
//! the power, an octave of keys and the octaves under it (the note), the sixteen channels, and the
//! velocity (fixed, with its dial, or from the hit's peak). Each note the trigger sends lights its
//! key for a moment, as bright as its velocity.

use mite_ui::kurbo::{Affine, Circle, Point, Rect, RoundedRect, Stroke};
use mite_ui::paint::{icon_button, raised};
use mite_ui::peniko::{Color, Fill};
use mite_ui::widgets::{Help, Knob, KnobKind};
use mite_ui::{write_if_changed, Align, DrawCx, MouseButton, ParamHost, TextStyle, UiEvent};

use crate::params::{MIDI_CHANNEL, MIDI_NOTE, MIDI_ON, MIDI_VELOCITY, MIDI_VEL_FIXED};

/// A five-pin DIN socket.
pub const MIDI_GLYPH: &str = "M12 3.5 A8.5 8.5 0 1 1 11.99 3.5 M6.8 12.5 a0.8 0.8 0 1 1 0.01 0 M16.4 12.5 a0.8 0.8 0 1 1 0.01 0 M8.2 8.6 a0.8 0.8 0 1 1 0.01 0 M15 8.6 a0.8 0.8 0 1 1 0.01 0 M11.6 7 a0.8 0.8 0 1 1 0.01 0 M10 17.5 H14";
/// Fixed velocity: bars all as tall; from the level: bars as tall as their hits.
const FIXED_GLYPH: &str = "M5 19 V9 M9.5 19 V9 M14 19 V9 M18.5 19 V9";
const LEVEL_GLYPH: &str = "M5 19 V15 M9.5 19 V6 M14 19 V12 M18.5 19 V9";
const POWER_GLYPH: &str = "M12 4 V11 M7.5 7.2 A6.5 6.5 0 1 0 16.5 7.2";

const WHITE: [usize; 7] = [0, 2, 4, 5, 7, 9, 11];
/// Black keys: pitch class and the white key they sit after.
const BLACK: [(usize, usize); 5] = [(1, 0), (3, 1), (6, 3), (8, 4), (10, 5)];
const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const OCTAVES: usize = 11;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Hot {
    Power,
    Key(usize),
    Octave(usize),
    Channel(usize),
    Velocity(usize),
}

pub struct MidiPanel {
    rect: Rect,
    knob: Knob,
    hot: Option<Hot>,
}

/// A note's name as REAPER shows it (60 = C4).
pub fn note_name(n: u8) -> String {
    format!("{}{}", NAMES[usize::from(n) % 12], i32::from(n) / 12 - 1)
}

/// The General MIDI drum a note plays, where there is one a trigger is commonly sent to.
fn gm_drum(n: u8) -> Option<&'static str> {
    Some(match n {
        35 => "acoustic bass drum",
        36 => "kick",
        37 => "side stick",
        38 => "snare",
        39 => "clap",
        40 => "electric snare",
        41 => "low floor tom",
        42 => "closed hi-hat",
        43 => "high floor tom",
        44 => "pedal hi-hat",
        45 => "low tom",
        46 => "open hi-hat",
        47 => "low-mid tom",
        48 => "high-mid tom",
        49 => "crash",
        50 => "high tom",
        51 => "ride",
        _ => return None,
    })
}

fn plain(host: &dyn ParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

fn set(host: &dyn ParamHost, id: &str, v: f32) {
    write_if_changed(host, id, host.normalize(id, v));
}

impl MidiPanel {
    pub fn new(rect: Rect) -> Self {
        let knob = Knob::new(MIDI_VEL_FIXED, "VELOCITY", KnobKind::Unipolar, Point::new(rect.x1 - 63.0, rect.y0 + 140.0), 46.0);
        Self { rect, knob, hot: None }
    }

    pub fn rect(&self) -> Rect {
        self.rect
    }

    pub fn busy(&self) -> bool {
        self.knob.is_dragging()
    }

    fn power_rect(&self) -> Rect {
        Rect::new(self.rect.x1 - 58.0, self.rect.y0 + 10.0, self.rect.x1 - 16.0, self.rect.y0 + 40.0)
    }

    fn keys_rect(&self) -> Rect {
        Rect::new(self.rect.x0 + 16.0, self.rect.y0 + 52.0, self.rect.x0 + 16.0 + 7.0 * 40.0, self.rect.y0 + 162.0)
    }

    fn white_rect(&self, i: usize) -> Rect {
        let k = self.keys_rect();
        let w = k.width() / 7.0;
        Rect::new(k.x0 + i as f64 * w, k.y0, k.x0 + (i + 1) as f64 * w, k.y1)
    }

    fn black_rect(&self, after: usize) -> Rect {
        let k = self.keys_rect();
        let w = k.width() / 7.0;
        let x = k.x0 + (after + 1) as f64 * w;
        Rect::new(x - 13.0, k.y0, x + 13.0, k.y0 + 68.0)
    }

    pub fn key_rect(&self, pc: usize) -> Rect {
        match BLACK.iter().find(|(p, _)| *p == pc) {
            Some(&(_, after)) => self.black_rect(after),
            None => self.white_rect(WHITE.iter().position(|&w| w == pc).unwrap_or(0)),
        }
    }

    pub fn octave_rect(&self, o: usize) -> Rect {
        let k = self.keys_rect();
        let w = k.width() / OCTAVES as f64;
        Rect::new(k.x0 + o as f64 * w + 1.5, k.y1 + 10.0, k.x0 + (o + 1) as f64 * w - 1.5, k.y1 + 38.0)
    }

    pub fn channel_rect(&self, c: usize) -> Rect {
        let x0 = self.keys_rect().x1 + 24.0;
        let (w, h) = (30.0, 36.0);
        let (col, row) = (c % 8, c / 8);
        let x = x0 + col as f64 * (w + 4.0);
        let y = self.rect.y0 + 52.0 + row as f64 * (h + 4.0);
        Rect::new(x, y, x + w, y + h)
    }

    pub fn velocity_rect(&self, i: usize) -> Rect {
        let x = self.rect.x1 - 106.0 + i as f64 * 46.0;
        Rect::new(x, self.rect.y0 + 52.0, x + 40.0, self.rect.y0 + 92.0)
    }

    pub fn power_button(&self) -> Rect {
        self.power_rect()
    }

    fn from_level(host: &dyn ParamHost) -> bool {
        plain(host, MIDI_VELOCITY.id).round() as i32 == 1
    }

    fn note(host: &dyn ParamHost) -> u8 {
        plain(host, MIDI_NOTE.id).round().clamp(0.0, 127.0) as u8
    }

    fn hot_at(&self, host: &dyn ParamHost, p: Point) -> Option<Hot> {
        if self.power_rect().contains(p) {
            return Some(Hot::Power);
        }
        // Black keys lie over the white ones.
        if let Some(&(pc, _)) = BLACK.iter().find(|(_, after)| self.black_rect(*after).contains(p)) {
            return Some(Hot::Key(pc));
        }
        if let Some(i) = (0..7).find(|&i| self.white_rect(i).contains(p)) {
            return Some(Hot::Key(WHITE[i]));
        }
        if let Some(o) = (0..OCTAVES).find(|&o| self.octave_rect(o).contains(p)) {
            return Some(Hot::Octave(o));
        }
        if let Some(c) = (0..16).find(|&c| self.channel_rect(c).contains(p)) {
            return Some(Hot::Channel(c));
        }
        if let Some(i) = (0..2).find(|&i| self.velocity_rect(i).contains(p)) {
            return Some(Hot::Velocity(i));
        }
        let _ = host;
        None
    }

    /// `last`: the last note sent, (its velocity 1…127, how much is left of its flash 0…1).
    pub fn draw(&self, cx: &mut DrawCx<'_>, last: Option<(f32, f32)>) {
        let flash = last.map_or(0.0, |(v, f)| v / 127.0 * f);
        let theme = cx.theme;
        let host = cx.host;
        let on = plain(host, MIDI_ON.id) >= 0.5;
        let accent = theme.accent;
        raised(cx.scene, RoundedRect::from_rect(self.rect, 12.0), theme.window, theme.panel_control_border, theme.shadows);
        let glyph = |cx: &mut DrawCx<'_>, d: &str, c: Point, size: f64, color: Color| super::draw_glyph(cx.scene, d, c, size, color, 1.6);
        glyph(cx, MIDI_GLYPH, Point::new(self.rect.x0 + 28.0, self.rect.y0 + 25.0), 20.0, accent);
        cx.text.draw(cx.scene, "MIDI trigger", self.rect.x0 + 46.0, self.rect.y0 + 18.0, TextStyle::BODY.weighted(700.0), theme.text_hi, Align::Left);
        let ink = icon_button(cx.scene, theme, self.power_rect(), theme.panel_control, self.hot == Some(Hot::Power), on, accent);
        glyph(cx, POWER_GLYPH, self.power_rect().center(), 20.0, ink);
        // Off, the rest is dimmed (still set-able: the note is chosen before it plays).
        let dim = if on { 1.0 } else { 0.45 };
        let note = Self::note(host);
        let (pc, oct) = (usize::from(note) % 12, usize::from(note) / 12);
        let key_fill = |cx: &mut DrawCx<'_>, r: Rect, black: bool, chosen: bool, hot: bool| {
            let shape = RoundedRect::from_rect(r, if black { 4.0 } else { 6.0 });
            let base = if black { theme.display } else { theme.panel_control };
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, base, None, &shape);
            // No glow: it is composited over everything and would light the black keys too.
            if chosen {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.3 * dim + 0.6 * flash), None, &shape);
                cx.scene.stroke(&Stroke::new(1.5 + 1.5 * f64::from(flash)), Affine::IDENTITY, accent.multiply_alpha(dim), None, &shape);
            } else {
                let border = if hot { accent } else { theme.panel_control_border };
                cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, border, None, &shape);
            }
        };
        for (i, &w) in WHITE.iter().enumerate() {
            key_fill(cx, self.white_rect(i).inflate(-1.5, 0.0), false, w == pc, self.hot == Some(Hot::Key(w)));
        }
        for &(p, after) in &BLACK {
            key_fill(cx, self.black_rect(after), true, p == pc, self.hot == Some(Hot::Key(p)));
        }
        for o in 0..OCTAVES {
            let r = self.octave_rect(o);
            let chosen = o == oct;
            let shape = RoundedRect::from_rect(r, 6.0);
            if chosen {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(0.25 * dim), None, &shape);
            }
            let border = if chosen || self.hot == Some(Hot::Octave(o)) { accent.multiply_alpha(dim) } else { theme.panel_control_border };
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, border, None, &shape);
            let ink = if chosen { accent } else { theme.text_lo };
            cx.text.draw(cx.scene, &format!("{}", o as i32 - 1), r.center().x, r.y0 + 8.0, TextStyle::LABEL.sized(10.0), ink, Align::Center);
        }
        let channel = plain(host, MIDI_CHANNEL.id).round() as usize;
        for c in 0..16 {
            let r = self.channel_rect(c);
            let chosen = c + 1 == channel;
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Channel(c)), chosen, accent.multiply_alpha(if chosen { dim } else { 1.0 }));
            cx.text.draw(cx.scene, &format!("{}", c + 1), r.center().x, r.y0 + 11.0, TextStyle::LABEL.sized(11.0), ink, Align::Center);
        }
        let level = Self::from_level(host);
        for (i, g) in [FIXED_GLYPH, LEVEL_GLYPH].iter().enumerate() {
            let r = self.velocity_rect(i);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, self.hot == Some(Hot::Velocity(i)), (i == 1) == level, accent);
            glyph(cx, g, r.center(), 24.0, ink);
        }
        if level {
            // From the level: the mapping, straight in dB from the threshold (1) to full scale
            // (127), and the last note on it.
            let k = self.knob.bounds();
            let (x0, x1, y0, y1) = (k.x0 + 2.0, k.x1 - 2.0, k.y0 + 4.0, k.y1 - 16.0);
            let frame = RoundedRect::new(x0 - 2.0, y0 - 2.0, x1 + 2.0, y1 + 2.0, 4.0);
            cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.panel_control_border, None, &frame);
            let at = |v: f64| Point::new(x0 + (x1 - x0) * (v - 1.0) / 126.0, y1 - (y1 - y0) * (v - 1.0) / 126.0);
            cx.scene.stroke(&Stroke::new(1.6), Affine::IDENTITY, accent.multiply_alpha(dim), None, &mite_ui::kurbo::Line::new(at(1.0), at(127.0)));
            if let Some((v, f)) = last.filter(|(_, f)| *f > 0.0) {
                cx.scene.fill(Fill::NonZero, Affine::IDENTITY, accent.multiply_alpha(f), None, &Circle::new(at(f64::from(v)), 3.5));
            }
            cx.text.draw(cx.scene, "VELOCITY", k.center().x, k.y1 - 10.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Center);
        } else {
            self.knob.draw(cx);
        }
    }

    pub fn help(&self, host: &dyn ParamHost) -> Option<Help> {
        let note = Self::note(host);
        let name = |n: u8| match gm_drum(n) {
            Some(d) => format!("{} · {n} ({d})", note_name(n)),
            None => format!("{} · {n}", note_name(n)),
        };
        Some(match self.hot? {
            Hot::Power => Help::default().text(if plain(host, MIDI_ON.id) >= 0.5 { "MIDI trigger on — a note at each opening, its note off at each closing (route the track's MIDI to a sampler)" } else { "MIDI trigger off — click to send a note at each opening" }),
            Hot::Key(pc) => {
                let n = ((usize::from(note) / 12) * 12 + pc).min(127) as u8;
                Help::default().text("Note ").slot(name(n), "D#9 · 127 (high floor tom)").text(" — wheel steps a semitone")
            }
            Hot::Octave(o) => {
                let n = (o * 12 + usize::from(note) % 12).min(127) as u8;
                Help::default().text("Octave — ").slot(name(n), "D#9 · 127 (high floor tom)")
            }
            Hot::Channel(c) => Help::default().text(format!("MIDI channel {} {}", c + 1, if c == 9 { "— General MIDI's drums" } else { "" })),
            Hot::Velocity(0) => Help::default().text("Fixed velocity — every note as loud, set by the dial"),
            Hot::Velocity(_) => Help::default().text("Velocity from the level — the hit's peak: 1 at the threshold, 127 at full scale. With 2 ms of lookahead the note still lands on the hit."),
        })
    }

    pub fn knob_help(&self, host: &dyn ParamHost, p: Option<Point>) -> Option<Help> {
        if Self::from_level(host) || !p.is_some_and(|p| self.knob.bounds().contains(p)) {
            return None;
        }
        let v = plain(host, MIDI_VEL_FIXED.id);
        Some(Help::default().text("Velocity ").slot(format!("{v:.0}"), "127").text(" — of every note"))
    }

    /// Handles `event` inside the panel (the caller keeps everything under it from reacting).
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        if !Self::from_level(host) && self.knob.event(event, host) {
            return true;
        }
        let note = Self::note(host);
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                let hot = self.hot_at(host, pos);
                let changed = hot != self.hot;
                self.hot = hot;
                changed
            }
            UiEvent::PointerLeft => {
                self.hot = None;
                true
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                match self.hot_at(host, pos) {
                    Some(Hot::Power) => set(host, MIDI_ON.id, if plain(host, MIDI_ON.id) >= 0.5 { 0.0 } else { 1.0 }),
                    Some(Hot::Key(pc)) => set(host, MIDI_NOTE.id, ((usize::from(note) / 12) * 12 + pc).min(127) as f32),
                    Some(Hot::Octave(o)) => set(host, MIDI_NOTE.id, (o * 12 + usize::from(note) % 12).min(127) as f32),
                    Some(Hot::Channel(c)) => set(host, MIDI_CHANNEL.id, (c + 1) as f32),
                    Some(Hot::Velocity(i)) => set(host, MIDI_VELOCITY.id, i as f32),
                    None => return false,
                }
                true
            }
            UiEvent::Wheel { pos, lines, .. } if self.keys_rect().union(self.octave_rect(OCTAVES - 1)).contains(pos) => {
                let step = if lines > 0.0 { 1.0 } else if lines < 0.0 { -1.0 } else { 0.0 };
                set(host, MIDI_NOTE.id, (f32::from(note) + step).clamp(0.0, 127.0));
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_named_as_reaper_names_them() {
        assert_eq!(note_name(60), "C4");
        assert_eq!(note_name(36), "C2");
        assert_eq!(note_name(38), "D2");
        assert_eq!(note_name(0), "C-1");
        assert_eq!(note_name(127), "G9");
    }
}
