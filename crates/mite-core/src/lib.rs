//! MiteFX core: the framework-free parameter model and lock-free audio → UI telemetry.
//!
//! Plugin crates declare their parameters once as [`ParamSpec`]s. `mite-plugin` turns them into
//! host parameters; tests turn them into the frozen golden manifest (BLUEPRINT §5 rule 5).

pub mod handover;
pub mod param;
pub mod registry;
pub mod telemetry;

pub use param::{manifest, BoolSpec, ChoiceSpec, DialCurve, ParamDisplay, ParamScale, ParamSpec};
pub use registry::Registry;
pub use handover::{triple_buffer, Handover, TripleReader, TripleWriter};
pub use telemetry::SpectrumTap;
pub use telemetry::{AtomicF32, StereoMeter};
