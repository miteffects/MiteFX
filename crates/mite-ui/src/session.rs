//! The session controls every plugin's header carries (BLUEPRINT §7.2, interaction.md's header):
//! presets as ‹ name › with the preset list and Save as…, undo and redo of the editor's own
//! edits, and snapshots A–D. A plugin hands it which parameters are sound (presets and undo cover
//! them) and which a snapshot holds, and routes its events through it.
//!
//! Undo covers the editor's own edits only: the view wraps the host in a
//! [`RecordingHost`](crate::RecordingHost) for its events and reports through [`Session::end`];
//! automation, the host's own undo and state loads just move the history's baseline
//! ([`Session::follow`]).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use mite_core::presets::{CurrentPreset, Preset, PresetLibrary, PresetValues};
use mite_core::snapshots::{self, Snapshots, Values, NAMES as SLOT_NAMES, SLOTS};
use mite_core::History;
use vello::kurbo::{Affine, BezPath, Cap, Circle, Join, Line, Point, Rect, RoundedRect, Stroke};
use vello::peniko::{Color, Fill};

use crate::host::{write_if_changed, ParamHost};
use crate::paint::{glass_panel, icon_button};
use crate::text::{Align, TextStyle};
use crate::view::{DrawCx, Key, Modifiers, MouseButton, UiEvent};
use crate::widgets::Help;

const UNDO_DEPTH: usize = 100;
const MENU_W: f64 = 320.0;
const ROW_H: f64 = 28.0;
/// Rows the preset list shows at once (the wheel scrolls the rest).
pub const MENU_ROWS: usize = 18;
const NAME_MAX: usize = 48;

/// What a plugin's session covers.
pub struct SessionConfig {
    /// The plugin's display name ("Mite EQ"): preset files carry it, and its folder is named so.
    pub plugin: &'static str,
    /// The built-in preset files, Init first.
    pub factory: &'static [&'static str],
    /// The sound parameters, which presets and undo cover (not bypass, delta, solo).
    pub sound_ids: Vec<String>,
    /// The parameters a snapshot holds.
    pub snapshot_ids: Vec<String>,
}

/// Where the header controls sit, in design units: x ranges (left, right) on one row.
#[derive(Debug, Clone, Copy)]
pub struct HeaderLayout {
    pub row: (f64, f64),
    pub prev: (f64, f64),
    pub name: (f64, f64),
    pub next: (f64, f64),
    pub undo: (f64, f64),
    pub redo: (f64, f64),
    /// Centre of the A button, the spacing of the letters, and their width.
    pub slot_x0: f64,
    pub slot_step: f64,
    pub slot_w: f64,
}

impl Default for HeaderLayout {
    /// The mockups' header (eq-main), shared by every plugin.
    fn default() -> Self {
        Self {
            row: (12.0, 44.0),
            prev: (150.0, 182.0),
            name: (186.0, 436.0),
            next: (440.0, 472.0),
            undo: (492.0, 524.0),
            redo: (528.0, 560.0),
            slot_x0: 604.0,
            slot_step: 38.0,
            slot_w: 32.0,
        }
    }
}

impl HeaderLayout {
    fn rect(&self, x: (f64, f64)) -> Rect {
        Rect::new(x.0, self.row.0, x.1, self.row.1)
    }

    pub fn slot_rect(&self, i: usize) -> Rect {
        let c = self.slot_x0 + self.slot_step * i as f64;
        Rect::new(c - self.slot_w / 2.0, self.row.0, c + self.slot_w / 2.0, self.row.1)
    }

    /// Row `k` of the open preset list, as shown (scrolling aside).
    pub fn menu_row(&self, k: usize) -> Rect {
        let y = self.row.1 + 6.0 + k as f64 * ROW_H;
        Rect::new(self.name.0, y, self.name.0 + MENU_W, y + ROW_H)
    }
}

/// One undo step: the sound parameters (normalized, in `sound_ids` order), the snapshots and the
/// current preset's name.
#[derive(Clone, PartialEq)]
struct Step {
    params: Vec<f32>,
    snapshots: snapshots::State,
    preset: Option<String>,
    /// The plugin's state beyond its parameters (`SessionExtra`).
    extra: Option<String>,
}

/// State beyond a plugin's parameters that its presets and undo carry too (Mite Rig's board).
pub trait SessionExtra: Send + Sync {
    /// The state now, as text.
    fn save(&self) -> String;
    /// Puts `text` back (None: the default, for a preset without any).
    fn load(&self, text: Option<&str>);
}

/// A row of the preset list.
#[derive(Clone, Copy, PartialEq)]
enum Row {
    /// "Save as…", or the name being typed.
    Save,
    Section(&'static str),
    /// Index into the library.
    Preset(usize),
}

/// The open preset list: the hovered row, the first row shown, and a name being typed.
struct PresetMenu {
    hover: Option<usize>,
    scroll: usize,
    naming: Option<String>,
}

pub struct Session {
    snapshots: Arc<Snapshots>,
    current: Arc<CurrentPreset>,
    library: PresetLibrary,
    sound_ids: Vec<String>,
    snapshot_ids: Vec<String>,
    layout: HeaderLayout,
    /// The values (normalized, by id) of the preset last loaded or saved, to show when the sound
    /// has moved on from it.
    loaded: Option<HashMap<String, f32>>,
    history: History<Step>,
    /// The state the last step ended in, and whether an edit of the editor's own is waiting to
    /// become a step.
    committed: Option<Step>,
    edited: bool,
    menu: Option<PresetMenu>,
    /// A snapshot letter being pressed (a click selects it, a drag onto another copies).
    slot_press: Option<usize>,
    pointer: Option<Point>,
    /// The parameters were replaced wholesale (a preset, a snapshot, undo) since last asked.
    replaced: bool,
    extra: Option<Arc<dyn SessionExtra>>,
    /// The extra state of the preset last loaded or saved.
    loaded_extra: Option<String>,
}

impl Session {
    /// A session over the plugin's snapshot and current-preset stores (the ones its parameters
    /// save with the state).
    /// Presets record the plugin's state version, and older ones are lifted as they are read
    /// (`PresetLibrary::with_state`).
    pub fn with_preset_state(mut self, current: u32, lift: mite_core::presets::Lift) -> Self {
        self.library.set_state(current, lift);
        self
    }

    pub fn new(config: SessionConfig, snapshots: Arc<Snapshots>, current: Arc<CurrentPreset>) -> Self {
        Self {
            snapshots,
            current,
            library: PresetLibrary::new(config.plugin, config.factory),
            sound_ids: config.sound_ids,
            snapshot_ids: config.snapshot_ids,
            layout: HeaderLayout::default(),
            loaded: None,
            history: History::new(UNDO_DEPTH),
            committed: None,
            edited: false,
            menu: None,
            slot_press: None,
            pointer: None,
            replaced: false,
            extra: None,
            loaded_extra: None,
        }
    }

    /// State beyond the parameters that presets and undo carry too.
    pub fn with_extra(mut self, extra: Arc<dyn SessionExtra>) -> Self {
        self.extra = Some(extra);
        self
    }

    fn extra_now(&self) -> Option<String> {
        self.extra.as_ref().map(|e| e.save())
    }

    /// Keeps the user's presets in `dir` instead of the settings folder (tests, a portable setup).
    pub fn with_preset_dir(mut self, dir: PathBuf) -> Self {
        self.library.set_dir(dir);
        self
    }

    pub fn with_layout(mut self, layout: HeaderLayout) -> Self {
        self.layout = layout;
        self
    }

    pub fn layout(&self) -> &HeaderLayout {
        &self.layout
    }

    pub fn snapshots(&self) -> &Snapshots {
        &self.snapshots
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// A letter is held (a drag may become a copy).
    pub fn busy(&self) -> bool {
        self.slot_press.is_some()
    }

    /// Whether the parameters were replaced wholesale since the last call (the view drops what
    /// it derived from them: a selection of a band now off, laid-out dials).
    pub fn take_replaced(&mut self) -> bool {
        std::mem::take(&mut self.replaced)
    }

    // ---- undo -------------------------------------------------------------------------------

    fn step(&self, host: &dyn ParamHost) -> Step {
        Step {
            params: self.sound_ids.iter().map(|id| host.normalized(id)).collect(),
            snapshots: self.snapshots.export(),
            preset: self.current.get(),
            extra: self.extra_now(),
        }
    }

    /// Before routing an event: makes sure there is a baseline to compare the edit with.
    pub fn begin(&mut self, host: &dyn ParamHost) {
        if self.committed.is_none() {
            self.committed = Some(self.step(host));
        }
    }

    /// After routing an event: `wrote` is whether the view's recording host saw a write, `busy`
    /// whether a gesture of the view's is still under way (a drag is one step when it ends).
    pub fn end(&mut self, host: &dyn ParamHost, wrote: bool, busy: bool) {
        self.edited |= wrote;
        // An edit of the extra state (the view's own: it changed during this event).
        if !self.edited && self.extra.is_some() {
            let now = self.extra_now();
            self.edited |= self.committed.as_ref().is_some_and(|c| c.extra != now);
        }
        if self.edited && !busy && !self.busy() {
            self.commit(host);
        }
    }

    /// On the UI timer: changes from outside the editor (automation, the host's own undo, a
    /// state load) are not undo steps; the baseline just follows them.
    pub fn follow(&mut self, host: &dyn ParamHost, busy: bool) {
        if !self.edited && !busy && !self.busy() {
            match &mut self.committed {
                Some(c) => {
                    for (v, id) in c.params.iter_mut().zip(&self.sound_ids) {
                        *v = host.normalized(id);
                    }
                    if let Some(e) = &self.extra {
                        c.extra = Some(e.save());
                    }
                }
                None => self.committed = Some(self.step(host)),
            }
        }
        if self.loaded.is_none() {
            if let Some(name) = self.current.get() {
                self.loaded = self.preset_values(host, &name);
            }
        }
    }

    /// Ends an edit: the state before it becomes an undo step.
    fn commit(&mut self, host: &dyn ParamHost) {
        let now = self.step(host);
        if let Some(before) = self.committed.take() {
            if before != now {
                self.history.push(before);
            }
        }
        self.committed = Some(now);
        self.edited = false;
    }

    /// Puts `state` back: the parameters that differ (as gestures), the snapshots, the preset.
    fn apply(&mut self, host: &dyn ParamHost, state: &Step) {
        for (id, &v) in self.sound_ids.iter().zip(&state.params) {
            write_if_changed(host, id, v);
        }
        self.snapshots.import(state.snapshots.clone());
        if let (Some(e), Some(text)) = (&self.extra, &state.extra) {
            e.load(Some(text));
        }
        self.current.set(state.preset.clone());
        self.loaded = state.preset.as_deref().and_then(|name| self.preset_values(host, name));
        self.committed = Some(state.clone());
        self.replaced = true;
    }

    pub fn undo(&mut self, host: &dyn ParamHost) {
        let now = self.step(host);
        if let Some(state) = self.history.undo(now) {
            self.apply(host, &state);
        }
    }

    pub fn redo(&mut self, host: &dyn ParamHost) {
        let now = self.step(host);
        if let Some(state) = self.history.redo(now) {
            self.apply(host, &state);
        }
    }

    // ---- snapshots --------------------------------------------------------------------------

    /// The snapshot parameters now.
    fn capture(&self, host: &dyn ParamHost) -> Values {
        self.snapshot_ids.iter().map(|id| (id.clone(), host.normalized(id))).collect()
    }

    fn recall(host: &dyn ParamHost, values: &Values) {
        for (id, &v) in values {
            write_if_changed(host, id, v);
        }
    }

    /// Makes `slot` the one being edited: the parameters now go into the slot that was, and
    /// `slot`'s values come back (a slot never stored starts as a copy).
    pub fn select_slot(&mut self, host: &dyn ParamHost, slot: usize) {
        let from = self.snapshots.active();
        if slot != from {
            let live = self.capture(host);
            self.snapshots.store(from, live.clone());
            match self.snapshots.slot(slot) {
                Some(values) => Self::recall(host, &values),
                None => self.snapshots.store(slot, live),
            }
            self.snapshots.set_active(slot);
        }
        self.replaced = true;
    }

    /// Copies snapshot `from` into `to` (the active one's contents are the parameters).
    pub fn copy_slot(&mut self, host: &dyn ParamHost, from: usize, to: usize) {
        let active = self.snapshots.active();
        let values = if from == active { self.capture(host) } else { self.snapshots.slot(from).unwrap_or_else(|| self.capture(host)) };
        if to == active {
            Self::recall(host, &values);
            self.replaced = true;
        }
        self.snapshots.store(to, values);
    }

    // ---- presets ----------------------------------------------------------------------------

    /// Where the current preset is in the list (Init until one is loaded, as the header says).
    fn preset_index(&self) -> Option<usize> {
        match self.current.get() {
            Some(name) => self.library.index_of(&name),
            None => (self.library.factory_len() > 0).then_some(0),
        }
    }

    /// The normalized values preset `name` sets, every sound parameter included (what it leaves
    /// out is at its default).
    fn preset_values(&self, host: &dyn ParamHost, name: &str) -> Option<HashMap<String, f32>> {
        let preset = self.library.find(name)?;
        Some(
            self.sound_ids
                .iter()
                .map(|id| {
                    let v = preset.values.get(id).map_or_else(|| host.default_normalized(id), |&plain| host.normalize(id, plain));
                    (id.clone(), v)
                })
                .collect(),
        )
    }

    /// Loads preset `i` into the snapshot being edited.
    pub fn load_preset(&mut self, host: &dyn ParamHost, i: usize) {
        let Some(name) = self.library.get(i).map(|p| p.name.clone()) else { return };
        let Some(values) = self.preset_values(host, &name) else { return };
        for id in &self.sound_ids {
            write_if_changed(host, id, values[id]);
        }
        if let Some(e) = &self.extra {
            let text = self.library.get(i).and_then(|p| p.extra.clone());
            e.load(text.as_deref());
            self.loaded_extra = Some(e.save());
        }
        self.current.set(Some(name));
        self.loaded = Some(values);
        self.edited = true;
        self.replaced = true;
    }

    /// Steps to the previous (-1) or next (+1) preset, wrapping.
    pub fn step_preset(&mut self, host: &dyn ParamHost, by: isize) {
        let n = self.library.len() as isize;
        if n == 0 {
            return;
        }
        let i = self.preset_index().map_or(if by > 0 { 0 } else { n - 1 }, |i| (i as isize + by).rem_euclid(n));
        self.load_preset(host, i as usize);
    }

    /// Saves the sound now as user preset `name` (only what differs from the defaults).
    pub fn save_preset(&mut self, host: &dyn ParamHost, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        let values: PresetValues = self
            .sound_ids
            .iter()
            .filter(|id| (host.normalized(id) - host.default_normalized(id)).abs() > 1e-7)
            .map(|id| (id.clone(), host.plain(id, host.normalized(id))))
            .collect();
        let extra = self.extra_now();
        if self.library.save_with(name, &values, extra.as_deref()).is_ok() {
            self.loaded_extra = extra;
            self.current.set(Some(name.to_owned()));
            self.loaded = self.preset_values(host, name);
            self.edited = true;
        }
    }

    /// Whether the sound has moved on from the preset last loaded or saved.
    pub fn preset_modified(&self, host: &dyn ParamHost) -> bool {
        self.loaded.as_ref().is_some_and(|values| self.sound_ids.iter().any(|id| (host.normalized(id) - values[id]).abs() > 1e-6))
            || (self.loaded.is_some() && self.loaded_extra.is_some() && self.loaded_extra != self.extra_now())
    }

    pub fn preset(&self, i: usize) -> Option<&Preset> {
        self.library.get(i)
    }

    fn rows(&self) -> Vec<Row> {
        let factory = self.library.factory_len();
        let mut rows = vec![Row::Save, Row::Section("FACTORY")];
        rows.extend((0..factory).map(Row::Preset));
        if self.library.user_len() > 0 {
            rows.push(Row::Section("YOURS"));
            rows.extend((factory..self.library.len()).map(Row::Preset));
        }
        rows
    }

    fn menu_bounds(&self) -> Rect {
        let n = self.rows().len().min(MENU_ROWS);
        let (first, last) = (self.layout.menu_row(0), self.layout.menu_row(n.saturating_sub(1)));
        Rect::new(first.x0, first.y0 - 4.0, first.x1, last.y1 + 4.0)
    }

    /// The row under `p`, as an index into [`Self::rows`].
    fn row_at(&self, p: Point) -> Option<usize> {
        let menu = self.menu.as_ref()?;
        let rows = self.rows().len();
        (0..rows.saturating_sub(menu.scroll).min(MENU_ROWS)).find(|&k| self.layout.menu_row(k).contains(p)).map(|k| k + menu.scroll)
    }

    pub fn open_menu(&mut self) {
        self.library.reload();
        let rows = self.rows().len();
        let scroll = self.preset_index().map_or(0, |i| (i + 3).saturating_sub(MENU_ROWS / 2)).min(rows.saturating_sub(MENU_ROWS));
        self.menu = Some(PresetMenu { hover: None, scroll, naming: None });
    }

    // ---- events -----------------------------------------------------------------------------

    /// Keys: typing a preset name, Esc on the list, undo and redo (Ctrl/Cmd+Z, Shift+Z, Y).
    /// `None`: not the session's; the view may use it.
    pub fn key(&mut self, key: Key, mods: Modifiers, host: &dyn ParamHost) -> Option<bool> {
        if let Some(name) = self.menu.as_mut().and_then(|m| m.naming.as_mut()) {
            match key {
                Key::Character(c) if !(mods.ctrl || mods.meta) => {
                    if name.chars().count() < NAME_MAX {
                        name.push(c);
                    }
                }
                Key::Backspace => {
                    name.pop();
                }
                Key::Enter => {
                    let name = name.clone();
                    self.save_preset(host, &name);
                    self.menu = None;
                }
                Key::Escape => {
                    if let Some(m) = &mut self.menu {
                        m.naming = None;
                    }
                }
                _ => return Some(false),
            }
            return Some(true);
        }
        match key {
            Key::Escape if self.menu.is_some() => {
                self.menu = None;
                Some(true)
            }
            Key::Character(c) if mods.ctrl || mods.meta => match c.to_ascii_lowercase() {
                'z' if mods.shift => {
                    self.redo(host);
                    Some(true)
                }
                'z' => {
                    self.undo(host);
                    Some(true)
                }
                'y' => {
                    self.redo(host);
                    Some(true)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Pointer events: the open preset list (modal), the header controls, and a letter being
    /// dragged. `None`: not the session's (the pointer is still tracked for hover).
    pub fn event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> Option<bool> {
        match *event {
            UiEvent::PointerMove { pos, .. } => self.pointer = Some(pos),
            UiEvent::PointerLeft => self.pointer = None,
            _ => {}
        }
        if self.menu.is_some() {
            return Some(self.menu_event(event, host));
        }
        let l = self.layout;
        match *event {
            UiEvent::PointerMove { .. } => self.slot_press.map(|_| true),
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                let at = |x: (f64, f64)| l.rect(x).contains(pos);
                if at(l.prev) {
                    self.step_preset(host, -1);
                } else if at(l.next) {
                    self.step_preset(host, 1);
                } else if at(l.name) {
                    self.open_menu();
                } else if at(l.undo) {
                    self.undo(host);
                } else if at(l.redo) {
                    self.redo(host);
                } else {
                    self.slot_press = Some((0..SLOTS).find(|&i| l.slot_rect(i).contains(pos))?);
                }
                Some(true)
            }
            UiEvent::PointerUp { pos, button: MouseButton::Left, .. } => {
                let from = self.slot_press.take()?;
                match (0..SLOTS).find(|&i| l.slot_rect(i).contains(pos)) {
                    Some(to) if to == from => self.select_slot(host, from),
                    Some(to) => self.copy_slot(host, from, to),
                    None => {}
                }
                Some(true)
            }
            _ => None,
        }
    }

    /// Events while the preset list is open: everything is the list's.
    fn menu_event(&mut self, event: &UiEvent, host: &dyn ParamHost) -> bool {
        match *event {
            UiEvent::PointerMove { pos, .. } => {
                let hover = self.row_at(pos);
                if let Some(m) = &mut self.menu {
                    m.hover = hover;
                }
            }
            UiEvent::PointerDown { pos, button: MouseButton::Left, .. } => {
                let rows = self.rows();
                match self.row_at(pos).map(|i| rows[i]) {
                    Some(Row::Save) => match self.menu.as_ref().and_then(|m| m.naming.clone()) {
                        // Clicking the row again (its tick) saves.
                        Some(name) => {
                            self.save_preset(host, &name);
                            self.menu = None;
                        }
                        None => {
                            let n = self.library.user_len() + 1;
                            let factory = self.library.factory_len();
                            let suggested = self.current.get().filter(|_| self.preset_index().is_some_and(|i| i >= factory)).unwrap_or(format!("My preset {n}"));
                            if let Some(m) = &mut self.menu {
                                m.naming = Some(suggested);
                            }
                        }
                    },
                    Some(Row::Preset(i)) => {
                        self.load_preset(host, i);
                        self.menu = None;
                    }
                    Some(Row::Section(_)) => {}
                    None => self.menu = None,
                }
            }
            UiEvent::Wheel { lines, .. } => {
                let max = self.rows().len().saturating_sub(MENU_ROWS);
                if let Some(m) = &mut self.menu {
                    m.scroll = (m.scroll as isize - lines.round() as isize).clamp(0, max as isize) as usize;
                }
            }
            UiEvent::PointerLeft => {
                if let Some(m) = &mut self.menu {
                    m.hover = None;
                }
            }
            _ => {}
        }
        true
    }

    /// The help strip's words for the header under the pointer.
    pub fn help(&self) -> Option<Help> {
        let p = self.pointer?;
        let l = &self.layout;
        let slot_at = |p: Point| (0..SLOTS).find(|&i| l.slot_rect(i).contains(p));
        if let Some(from) = self.slot_press {
            if let Some(to) = slot_at(p).filter(|&to| to != from) {
                return Some(Help::default().text(format!("Copy {} to {} — release to copy", SLOT_NAMES[from], SLOT_NAMES[to])));
            }
        }
        let i = slot_at(p)?;
        let store = &self.snapshots;
        let state = if store.active() == i { " · editing" } else if store.filled(i) { "" } else { " · empty (starts as a copy)" };
        Some(Help::default().text(format!("Snapshot {}{state} — click to edit it; drag onto another letter to copy it there", SLOT_NAMES[i])))
    }

    // ---- drawing ----------------------------------------------------------------------------

    /// The header controls: ‹ name ›, undo and redo, the letters A–D.
    pub fn draw(&self, cx: &mut DrawCx<'_>) {
        let theme = cx.theme;
        let l = self.layout;
        let (y0, _) = l.row;
        let hot = |r: Rect| self.pointer.is_some_and(|p| r.contains(p));
        let round = Stroke::new(1.5).with_caps(Cap::Round).with_join(Join::Round);

        // Presets: ‹ name ›.
        for (x, dir) in [(l.prev, -1.0), (l.next, 1.0)] {
            let r = l.rect(x);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, hot(r), false, theme.accent);
            let c = r.center();
            let mut chevron = BezPath::new();
            chevron.move_to((c.x - 2.0 * dir, c.y - 5.0));
            chevron.line_to((c.x + 2.5 * dir, c.y));
            chevron.line_to((c.x - 2.0 * dir, c.y + 5.0));
            cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &chevron);
        }
        let name_rect = l.rect(l.name);
        let ink = icon_button(cx.scene, theme, name_rect, theme.panel_control, hot(name_rect), self.menu.is_some(), theme.accent);
        let lens = Point::new(name_rect.x0 + 16.0, name_rect.center().y - 1.0);
        cx.scene.stroke(&Stroke::new(1.4), Affine::IDENTITY, ink, None, &Circle::new(lens, 4.0));
        cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &Line::new(lens + (2.9, 2.9), lens + (6.0, 6.0)));
        let name = self.current.get().unwrap_or_else(|| "Init".to_owned());
        let width = cx.text.draw(cx.scene, &name, name_rect.x0 + 30.0, y0 + 9.0, TextStyle::BODY.weighted(600.0), theme.text_hi, Align::Left);
        if self.preset_modified(cx.host) {
            // The sound has moved on from the preset: a dot after its name.
            let dot = Point::new(name_rect.x0 + 30.0 + f64::from(width) + 7.0, name_rect.center().y);
            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent, None, &Circle::new(dot, 2.5));
        }

        // Undo and redo: curved arrows, dim when there is nothing to go back or forward to.
        // `dir` is the side the arrow's tip points to: left for undo, right for redo.
        for (x, dir, has) in [(l.undo, 1.0, self.history.can_undo()), (l.redo, -1.0, self.history.can_redo())] {
            let r = l.rect(x);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, has && hot(r), false, theme.accent);
            let ink = if has { ink } else { theme.text_disabled };
            let c = r.center();
            let mut arrow = BezPath::new();
            arrow.move_to((c.x + 5.0 * dir, c.y + 5.0));
            arrow.curve_to((c.x + 6.0 * dir, c.y - 3.0), (c.x - 1.0 * dir, c.y - 5.0), (c.x - 5.0 * dir, c.y - 2.0));
            arrow.move_to((c.x - 5.0 * dir, c.y - 6.5));
            arrow.line_to((c.x - 5.0 * dir, c.y - 2.0));
            arrow.line_to((c.x - 0.5 * dir, c.y - 1.5));
            cx.scene.stroke(&round, Affine::IDENTITY, ink, None, &arrow);
        }

        // Snapshots: the letters A–D, the edited one lit, empty ones dim.
        let active = self.snapshots.active();
        for (i, name) in SLOT_NAMES.iter().enumerate() {
            let r = l.slot_rect(i);
            let target = self.slot_press.is_some_and(|from| from != i) && hot(r);
            let ink = icon_button(cx.scene, theme, r, theme.panel_control, target || hot(r), active == i, theme.accent);
            let ink = if active != i && !hot(r) && !self.snapshots.filled(i) { theme.text_lo } else { ink };
            cx.text.draw(cx.scene, name, r.center().x, y0 + 9.0, TextStyle::LABEL.sized(11.0), ink, Align::Center);
        }
    }

    /// The open preset list, drawn over everything: "Save as…" (or the name being typed), then
    /// the factory presets and the user's.
    pub fn draw_menu(&self, cx: &mut DrawCx<'_>) {
        let Some(menu) = &self.menu else { return };
        let theme = cx.theme;
        let bounds = self.menu_bounds();
        if theme.shadows {
            cx.scene.draw_blurred_rounded_rect(Affine::translate((0.0, 10.0)), bounds, Color::from_rgba8(0, 0, 0, 140), 10.0, 16.0);
        }
        glass_panel(cx, RoundedRect::from_rect(bounds, 10.0), theme.blur_menu);
        let current = self.preset_index();
        for (k, row) in self.rows().iter().enumerate().skip(menu.scroll).take(MENU_ROWS) {
            let r = self.layout.menu_row(k - menu.scroll).inset((-4.0, 0.0));
            let hovered = menu.hover == Some(k);
            match *row {
                Row::Save => {
                    let shape = RoundedRect::from_rect(r.inset((0.0, -2.0)), 6.0);
                    if let Some(name) = &menu.naming {
                        // The name field: the text, a caret, and a tick that saves.
                        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.well, None, &shape);
                        cx.scene.stroke(&Stroke::new(1.0), Affine::IDENTITY, theme.accent, None, &shape);
                        let w = cx.text.draw(cx.scene, name, r.x0 + 10.0, r.y0 + 7.0, TextStyle::BODY, theme.text_strong, Align::Left);
                        let caret = r.x0 + 11.0 + f64::from(w);
                        cx.scene.stroke(&Stroke::new(1.2), Affine::IDENTITY, theme.accent, None, &Line::new((caret, r.y0 + 7.0), (caret, r.y1 - 7.0)));
                        let k = Point::new(r.x1 - 16.0, r.center().y);
                        let mut tick = BezPath::new();
                        tick.move_to((k.x - 5.0, k.y));
                        tick.line_to((k.x - 1.5, k.y + 3.5));
                        tick.line_to((k.x + 5.0, k.y - 3.5));
                        let stroke = Stroke::new(1.6).with_caps(Cap::Round).with_join(Join::Round);
                        cx.scene.stroke(&stroke, Affine::IDENTITY, theme.accent, None, &tick);
                    } else {
                        if hovered {
                            cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(0.16), None, &shape);
                        }
                        let ink = if hovered { theme.text_hi } else { theme.text_mid };
                        cx.text.draw(cx.scene, "Save as…", r.x0 + 10.0, r.y0 + 7.0, TextStyle::BODY.weighted(600.0), ink, Align::Left);
                    }
                }
                Row::Section(label) => {
                    cx.text.draw(cx.scene, label, r.x0 + 10.0, r.y0 + 10.0, TextStyle::LABEL.sized(9.0), theme.text_lo, Align::Left);
                }
                Row::Preset(i) => {
                    let Some(p) = self.library.get(i) else { continue };
                    let on = current == Some(i);
                    let shape = RoundedRect::from_rect(r.inset((0.0, -2.0)), 6.0);
                    if on || hovered {
                        cx.scene.fill(Fill::NonZero, Affine::IDENTITY, theme.accent.multiply_alpha(if on { 0.2 } else { 0.12 }), None, &shape);
                    }
                    let ink = if on { theme.text_strong } else if hovered { theme.text_hi } else { theme.text_mid };
                    cx.text.draw(cx.scene, &p.name, r.x0 + 10.0, r.y0 + 7.0, TextStyle::BODY, ink, Align::Left);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{MemoryParamHost, RecordingHost};
    use mite_core::{ParamDisplay, ParamScale, ParamSpec};

    const GAIN: ParamSpec = ParamSpec {
        id: "gain",
        name: "Gain",
        unit: "dB",
        min: -24.0,
        max: 24.0,
        default: 0.0,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 1,
        wheel_step: 0.5,
        wheel_step_fine: 0.1,
        display: ParamDisplay::Number,
    };
    const LOUD: &str = r#"{"plugin":"Test","version":1,"name":"Loud","params":{"gain":12.0}}"#;
    const INIT: &str = r#"{"plugin":"Test","version":1,"name":"Init","params":{}}"#;

    fn session() -> (Session, MemoryParamHost) {
        let config = SessionConfig { plugin: "Test", factory: &[INIT, LOUD], sound_ids: vec!["gain".into()], snapshot_ids: vec!["gain".into()] };
        let dir = std::env::temp_dir().join(format!("mitefx-session-{}", std::process::id()));
        let s = Session::new(config, Arc::new(Snapshots::new()), Arc::new(CurrentPreset::default())).with_preset_dir(dir);
        (s, MemoryParamHost::new().with(GAIN))
    }

    /// An edit as a view makes it: through a recording host, then reported.
    fn edit(s: &mut Session, host: &MemoryParamHost, plain: f32) {
        s.begin(host);
        let rec = RecordingHost::new(host);
        rec.set_normalized("gain", GAIN.normalize(plain));
        s.end(host, rec.wrote(), false);
    }

    #[test]
    fn edits_undo_and_redo_but_outside_changes_do_not() {
        let (mut s, host) = session();
        edit(&mut s, &host, 6.0);
        edit(&mut s, &host, 9.0);
        // Automation moves it: not a step, just the new baseline.
        host.set_plain("gain", 3.0);
        s.follow(&host, false);
        s.undo(&host);
        assert!((host.plain("gain", host.normalized("gain")) - 6.0).abs() < 1e-4);
        s.redo(&host);
        assert!((host.plain("gain", host.normalized("gain")) - 3.0).abs() < 1e-4, "redo returns to the state undo left");
    }

    #[test]
    fn a_preset_loads_its_values_and_the_header_notices_when_the_sound_moves_on() {
        let (mut s, host) = session();
        s.begin(&host);
        s.load_preset(&host, 1);
        s.end(&host, false, false);
        assert!((host.plain("gain", host.normalized("gain")) - 12.0).abs() < 1e-4);
        assert!(s.take_replaced());
        assert!(!s.preset_modified(&host));
        edit(&mut s, &host, 2.0);
        assert!(s.preset_modified(&host));
        s.undo(&host);
        assert!(!s.preset_modified(&host));
    }

    #[test]
    fn snapshots_switch_and_copy() {
        let (mut s, host) = session();
        edit(&mut s, &host, 5.0);
        s.select_slot(&host, 1); // B starts as a copy of A
        edit(&mut s, &host, -5.0);
        s.select_slot(&host, 0);
        assert!((host.plain("gain", host.normalized("gain")) - 5.0).abs() < 1e-4);
        s.copy_slot(&host, 1, 0); // B onto A, the one being edited
        assert!((host.plain("gain", host.normalized("gain")) + 5.0).abs() < 1e-4);
    }
}
