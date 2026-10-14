//! MiteFX core: the framework-free parameter model and lock-free audio → UI telemetry.
//!
//! Plugin crates declare their parameters once as [`ParamSpec`]s. `mite-plugin` turns them into
//! host parameters; tests turn them into the frozen golden manifest (BLUEPRINT §5 rule 5).

pub mod param;
pub mod telemetry;

pub use param::{manifest, BoolSpec, ChoiceSpec, ParamDisplay, ParamScale, ParamSpec};
pub use telemetry::SpectrumTap;
pub use telemetry::{AtomicF32, StereoMeter};
