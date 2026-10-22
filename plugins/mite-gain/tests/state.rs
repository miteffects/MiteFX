//! Golden plugin states (BLUEPRINT §5), checked through the real CLAP entry point by
//! `mite_plugin::testing::check_golden_states`. When `STATE_VERSION` goes up, keep the old file,
//! add its `.expected.json`, and bless the new one with `MITEFX_BLESS=1`.

use std::path::Path;

use mite_gain::params::STATE_VERSION;
use mite_plugin::testing::{check_golden_states, ClapInstance};
use nih_plug::prelude::ClapPlugin;
use serde_json::{json, Value};

#[test]
fn every_golden_state_loads_and_saves_back() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden");
    check_golden_states(&mite_gain::clap_entry, mite_gain::MiteGain::CLAP_ID, &dir, "mite_gain", STATE_VERSION, || {
        let plugin = ClapInstance::new(&mite_gain::clap_entry, mite_gain::MiteGain::CLAP_ID);
        let mut state: Value = serde_json::from_slice(&plugin.save()).expect("JSON");
        state["params"]["gain"] = json!({"f32": -7.5});
        assert!(plugin.load(&serde_json::to_vec(&state).expect("JSON")));
        plugin.save()
    });
}
