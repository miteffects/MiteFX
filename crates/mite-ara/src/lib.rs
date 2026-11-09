//! MiteFX's ARA 2 layer (docs/design/pitch-brief.md §1a): the ARA C API (`sys`, generated), its
//! CLAP binding (`clap`), and the plug-in side of a document (`document`). No plug-in framework:
//! the nih-plug fork only hands the host's calls through.

pub mod clap;
pub mod document;
#[allow(clippy::all, clippy::pedantic)]
pub mod sys;
pub mod testing;

pub use document::{bind, document, factory, AudioModification, AudioSource, Document, FactoryInfo, Instance, InstanceState, Model, MusicalContext, PlaybackRegion};
