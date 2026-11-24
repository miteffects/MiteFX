//! Blocks a host runs: [`Kind::Plugin`](crate::blocks::Kind::Plugin).
//!
//! Mite Rig keeps such a block on the board like any other (its place, bypass, meters and an
//! opaque `ext` value the host stores with it) and asks the host set here for its audio unit.
//! Miteboard sets one that runs a VST3 in its own process. Without a host (Mite Rig as a plugin in
//! a DAW) the block passes its input through unchanged.

use std::sync::{Arc, RwLock};

use crate::blocks::Unit;
use crate::board::Live;

pub trait ExternalHost: Send + Sync {
    /// The audio unit for block `live` at `fs` (worker thread; may allocate, must not block).
    fn unit(&self, live: &Live, fs: f32) -> Box<dyn Unit>;
}

static HOST: RwLock<Option<Arc<dyn ExternalHost>>> = RwLock::new(None);

/// The host that runs plugin blocks from now on (`None`: they pass through).
pub fn set_host(host: Option<Arc<dyn ExternalHost>>) {
    *HOST.write().unwrap_or_else(|e| e.into_inner()) = host;
}

pub(crate) fn unit(live: &Live, fs: f32) -> Box<dyn Unit> {
    match HOST.read().unwrap_or_else(|e| e.into_inner()).as_ref() {
        Some(h) => h.unit(live, fs),
        None => Box::new(Through),
    }
}

/// A plugin block with no host to run it: what comes in goes out.
struct Through;

impl Unit for Through {
    fn process(&mut self, _l: &mut [f32], _r: &mut [f32], _live: &Live) {}
    fn reset(&mut self, _live: &Live) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Kind;
    use crate::board::Board;

    #[test]
    fn a_plugin_block_round_trips_with_its_host_data_and_passes_through_without_a_host() {
        let json = r#"{"items":[{"block":{"id":3,"kind":"plugin","ext":{"path":"/x/Blackhole.vst3","state":"AAEC"}}}]}"#;
        let board = Board::new();
        board.load_json(Some(json));
        let b = board.block(3).expect("the plugin block");
        assert_eq!(b.kind(), Kind::Plugin);
        let doc: serde_json::Value = serde_json::from_str(&board.to_json()).unwrap();
        assert_eq!(doc["items"][0]["block"]["ext"]["path"], "/x/Blackhole.vst3");
        let mut u = unit(&b.live, 48_000.0);
        let (mut l, mut r) = (vec![0.5f32; 64], vec![-0.5f32; 64]);
        u.process(&mut l, &mut r, &b.live);
        assert!(l.iter().all(|&x| x == 0.5) && r.iter().all(|&x| x == -0.5));
    }
}
