//! Snapshots A–D (BLUEPRINT §7.2 "A/B/C/D snapshots and copy between them").
//!
//! One slot is active: the band parameters are its contents, so editing always edits it. The
//! other slots are stored as the band parameters' normalized values by id, which is also what the
//! plugin state saves. Switching slots writes the stored values back into the parameters, so the
//! audio thread only ever plays the parameters. A slot never stored is a copy of the active one.
//! Everything here runs on the UI or host thread.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::params::{band_id, BANDS, SNAPSHOT_PARAMS};

pub const SLOTS: usize = 4;
pub const NAMES: [&str; SLOTS] = ["A", "B", "C", "D"];

/// A stored slot: each band parameter's normalized value, by id (`b03_freq` …).
pub type Values = BTreeMap<String, f32>;

/// Every snapshot parameter id, in order.
pub fn snapshot_ids() -> impl Iterator<Item = String> {
    (0..BANDS).flat_map(|b| SNAPSHOT_PARAMS.iter().map(move |p| band_id(b, p)))
}

/// The saved form of the snapshots.
#[derive(Default, Clone, Serialize, Deserialize)]
struct State {
    active: usize,
    slots: [Option<Values>; SLOTS],
}

/// The snapshot store, shared by the parameters (saving) and the editor (switching, copying).
#[derive(Default)]
pub struct Snapshots {
    state: Mutex<State>,
}

impl Snapshots {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn active(&self) -> usize {
        self.state().active
    }

    /// Whether `slot` has been stored.
    pub fn filled(&self, slot: usize) -> bool {
        self.state().slots[slot].is_some()
    }

    pub fn slot(&self, slot: usize) -> Option<Values> {
        self.state().slots[slot].clone()
    }

    pub fn store(&self, slot: usize, values: Values) {
        self.state().slots[slot] = Some(values);
    }

    pub fn set_active(&self, slot: usize) {
        self.state().active = slot.min(SLOTS - 1);
    }

    /// The saved form, with `live` (the band parameters now) as the active slot.
    pub fn to_json(&self, live: Values) -> String {
        let mut state = self.state().clone();
        state.slots[state.active] = Some(live);
        serde_json::to_string(&state).unwrap_or_default()
    }

    /// Restores the saved form. Unreadable input leaves the store as it was.
    pub fn load_json(&self, json: &str) {
        let Ok(mut state) = serde_json::from_str::<State>(json) else { return };
        state.active = state.active.min(SLOTS - 1);
        *self.state() = state;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_saved_form_round_trips_and_the_active_slot_is_the_live_one() {
        let s = Snapshots::new();
        let values = |gain: f32| -> Values { [("b01_gain".to_owned(), gain)].into_iter().collect() };
        s.store(2, values(0.25));
        s.set_active(1);
        let json = s.to_json(values(0.75));
        let t = Snapshots::new();
        t.load_json(&json);
        assert_eq!(t.active(), 1);
        assert_eq!(t.slot(1), Some(values(0.75)));
        assert_eq!(t.slot(2), Some(values(0.25)));
        assert!(!t.filled(0) && !t.filled(3));
        // Garbage leaves the store alone.
        t.load_json("not json");
        assert_eq!(t.active(), 1);
    }
}
