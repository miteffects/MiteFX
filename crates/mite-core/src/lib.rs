//! MiteFX core: the framework-free parameter model, lock-free audio → UI telemetry, and what
//! every plugin's session shares (presets, snapshots A–D, undo history).
//!
//! Plugin crates declare their parameters once as [`ParamSpec`]s. `mite-plugin` turns them into
//! host parameters; tests turn them into the frozen golden manifest (BLUEPRINT §5 rule 5).

pub mod handover;
pub mod modmatrix;
pub mod param;
pub mod presets;
pub mod registry;
pub mod snapshots;
pub mod telemetry;
pub mod undo;

pub use param::{manifest, BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
pub use registry::Registry;
pub use undo::History;
pub use handover::{triple_buffer, Handover, TripleReader, TripleWriter};
pub use telemetry::{HistoryTap, SpectrumTap};
pub use telemetry::{AtomicF32, StereoMeter};
