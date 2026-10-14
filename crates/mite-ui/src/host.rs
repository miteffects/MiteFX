//! The UI's view of plugin parameters.

use std::cell::RefCell;
use std::collections::HashMap;

use mite_core::ParamSpec;

/// Parameter access for views. Values are normalised 0–1; formatting uses the plugin's
/// `mite_core::ParamSpec`.
///
/// Every edit is a gesture — `begin_gesture`, any number of `set_normalized`, `end_gesture` — so
/// hosts record one automation/undo step per drag (BLUEPRINT §5 rule 11).
pub trait ParamHost {
    fn normalized(&self, id: &str) -> f32;
    /// Plain value for a normalised value, using the host parameter's own mapping.
    fn plain(&self, id: &str, normalized: f32) -> f32;
    fn default_normalized(&self, id: &str) -> f32;
    fn begin_gesture(&self, id: &str);
    fn set_normalized(&self, id: &str, normalized: f32);
    fn end_gesture(&self, id: &str);
}

/// In-memory host for the gallery, snapshots and tests.
#[derive(Debug, Default)]
pub struct MemoryParamHost {
    /// id → (spec, current normalised value)
    values: RefCell<HashMap<&'static str, (ParamSpec, f32)>>,
}

impl MemoryParamHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a parameter at its default.
    pub fn with(self, spec: ParamSpec) -> Self {
        self.values.borrow_mut().insert(spec.id, (spec, spec.normalize(spec.default)));
        self
    }

    /// Sets a plain value, e.g. to stage a snapshot.
    pub fn set_plain(&self, id: &str, plain: f32) {
        if let Some((spec, v)) = self.values.borrow_mut().get_mut(id) {
            *v = spec.normalize(plain);
        }
    }
}

impl ParamHost for MemoryParamHost {
    fn normalized(&self, id: &str) -> f32 {
        self.values.borrow().get(id).map_or(0.0, |v| v.1)
    }

    fn plain(&self, id: &str, normalized: f32) -> f32 {
        self.values.borrow().get(id).map_or(0.0, |v| v.0.denormalize(normalized))
    }

    fn default_normalized(&self, id: &str) -> f32 {
        self.values.borrow().get(id).map_or(0.0, |v| v.0.normalize(v.0.default))
    }

    fn begin_gesture(&self, _id: &str) {}

    fn set_normalized(&self, id: &str, normalized: f32) {
        if let Some(v) = self.values.borrow_mut().get_mut(id) {
            v.1 = normalized.clamp(0.0, 1.0);
        }
    }

    fn end_gesture(&self, _id: &str) {}
}
