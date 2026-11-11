//! The UI's view of plugin parameters.

use std::cell::{Cell, RefCell};
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
    /// Normalised value for a plain value, using the host parameter's own mapping.
    fn normalize(&self, id: &str, plain: f32) -> f32;
    fn default_normalized(&self, id: &str) -> f32;
    fn begin_gesture(&self, id: &str);
    fn set_normalized(&self, id: &str, normalized: f32);
    fn end_gesture(&self, id: &str);
}

/// The host as a view's own events see it: everything passes through, and a write is noted, so
/// the edit can become an undo step (automation and host changes never pass through here).
pub struct RecordingHost<'a> {
    inner: &'a dyn ParamHost,
    wrote: Cell<bool>,
}

impl<'a> RecordingHost<'a> {
    pub fn new(inner: &'a dyn ParamHost) -> Self {
        Self { inner, wrote: Cell::new(false) }
    }

    /// Whether anything was written through it.
    pub fn wrote(&self) -> bool {
        self.wrote.get()
    }
}

impl ParamHost for RecordingHost<'_> {
    fn normalized(&self, id: &str) -> f32 {
        self.inner.normalized(id)
    }
    fn plain(&self, id: &str, normalized: f32) -> f32 {
        self.inner.plain(id, normalized)
    }
    fn normalize(&self, id: &str, plain: f32) -> f32 {
        self.inner.normalize(id, plain)
    }
    fn default_normalized(&self, id: &str) -> f32 {
        self.inner.default_normalized(id)
    }
    fn begin_gesture(&self, id: &str) {
        self.inner.begin_gesture(id);
    }
    fn set_normalized(&self, id: &str, normalized: f32) {
        self.wrote.set(true);
        self.inner.set_normalized(id, normalized);
    }
    fn end_gesture(&self, id: &str) {
        self.inner.end_gesture(id);
    }
}

/// Writes `normalized` to `id` as one gesture, if it differs (so recalls and loads only touch
/// what changes, and host undo sees each as a step).
pub fn write_if_changed(host: &dyn ParamHost, id: &str, normalized: f32) {
    if (host.normalized(id) - normalized).abs() > 1e-7 {
        host.begin_gesture(id);
        host.set_normalized(id, normalized);
        host.end_gesture(id);
    }
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

    /// Adds a parameter under a different id (e.g. a per-band `b03_freq` from a `freq` spec).
    pub fn with_id(self, id: &'static str, spec: ParamSpec) -> Self {
        let spec = ParamSpec { id, ..spec };
        self.with(spec)
    }

    /// A parameter's spec (its id, range, scale and default).
    pub fn spec(&self, id: &str) -> Option<ParamSpec> {
        self.values.borrow().get(id).map(|v| v.0)
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

    fn normalize(&self, id: &str, plain: f32) -> f32 {
        self.values.borrow().get(id).map_or(0.0, |v| v.0.normalize(plain))
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
