//! State migrations (`mite_plugin::state::migrate`, from `Plugin::filter_state`).
//!
//! **1 → 2 (2026-10-23): thresholds and knees are on the input.** Version 1 kept a stage's
//! threshold and knee in the level the stages below it left, so changing a lower stage moved where
//! a higher one started, and a knee behind a hard stage reached far below its threshold. Version
//! 2 sets them on the input. The step puts every threshold at the input level where version 1's
//! stage started acting and every knee at the width of input it spanned, so the curve is the
//! same; the parameters and every stored snapshot are lifted.

use mite_core::ParamSpec;
use mite_dsp::dynamics::{Quadrant, STAGES};
use nih_plug::wrapper::state::{ParamValue, PluginState};

use crate::params::{stage_id, S_KNEE, S_ON, S_QUAD, S_RANGE, S_RATIO, S_THRESH};

/// One stage's curve fields: on, quadrant, threshold, ratio, knee, range (plain values).
type Fields = (bool, usize, f32, f32, f32, f32);

pub fn v1_to_v2(state: &mut PluginState) {
    let params = &mut state.params;
    let f = |params: &std::collections::BTreeMap<String, ParamValue>, slot: usize, spec: &ParamSpec| match params.get(&stage_id(slot, spec.id)) {
        Some(ParamValue::F32(v)) => *v,
        Some(ParamValue::I32(v)) => *v as f32,
        Some(ParamValue::Bool(v)) => f32::from(u8::from(*v)),
        _ => spec.default,
    };
    let flag = |params: &std::collections::BTreeMap<String, ParamValue>, slot: usize, id: &str| match params.get(&stage_id(slot, id)) {
        Some(ParamValue::Bool(v)) => *v,
        Some(ParamValue::F32(v)) => *v >= 0.5,
        Some(ParamValue::I32(v)) => *v != 0,
        _ => false,
    };
    let quad = |params: &std::collections::BTreeMap<String, ParamValue>, slot: usize| match params.get(&stage_id(slot, S_QUAD.id)) {
        Some(ParamValue::I32(v)) => (*v).max(0) as usize,
        Some(ParamValue::F32(v)) => v.round().max(0.0) as usize,
        _ => 0,
    };
    let old: [Fields; STAGES] = std::array::from_fn(|i| (flag(params, i, S_ON.id), quad(params, i), f(params, i, &S_THRESH), f(params, i, &S_RATIO), f(params, i, &S_KNEE), f(params, i, &S_RANGE)));
    for (slot, (threshold, knee)) in on_the_input(&old).into_iter().enumerate() {
        if params.contains_key(&stage_id(slot, S_THRESH.id)) {
            params.insert(stage_id(slot, S_THRESH.id), ParamValue::F32(threshold));
        }
        if params.contains_key(&stage_id(slot, S_KNEE.id)) {
            params.insert(stage_id(slot, S_KNEE.id), ParamValue::F32(knee));
        }
    }
    if let Some(json) = state.fields.get_mut(mite_plugin::state::SNAPSHOTS_KEY) {
        if let Some(lifted) = lift_snapshots(json) {
            *json = lifted;
        }
    }
}

/// The snapshots' saved form with every slot lifted (its values are normalized). A slot keeps
/// only what it stored.
fn lift_snapshots(json: &str) -> Option<String> {
    let mut state: serde_json::Value = serde_json::from_str(json).ok()?;
    for slot in state.get_mut("slots")?.as_array_mut()?.iter_mut() {
        let Some(values) = slot.as_object_mut() else { continue };
        let get = |i: usize, id: &str| values.get(&stage_id(i, id)).and_then(serde_json::Value::as_f64).map(|v| v as f32);
        let plain = |i: usize, spec: &ParamSpec| get(i, spec.id).map_or(spec.default, |n| spec.denormalize(n));
        let old: [Fields; STAGES] = std::array::from_fn(|i| (get(i, S_ON.id).unwrap_or(0.0) >= 0.5, (get(i, S_QUAD.id).unwrap_or(0.0) * 3.0).round().max(0.0) as usize, plain(i, &S_THRESH), plain(i, &S_RATIO), plain(i, &S_KNEE), plain(i, &S_RANGE)));
        for (i, (threshold, knee)) in on_the_input(&old).into_iter().enumerate() {
            if values.contains_key(&stage_id(i, S_THRESH.id)) {
                values.insert(stage_id(i, S_THRESH.id), S_THRESH.normalize(threshold).into());
            }
            if values.contains_key(&stage_id(i, S_KNEE.id)) {
                values.insert(stage_id(i, S_KNEE.id), S_KNEE.normalize(knee).into());
            }
        }
    }
    serde_json::to_string(&state).ok()
}

/// State 2 → 3: each stage gained an auto release switch. Absent from an older state, it reads
/// as off, so the state sounds as it did: nothing to change.
pub fn v2_to_v3(_state: &mut PluginState) {}

/// Version 2's (threshold, knee) per stage: the input level where each of version 1's stages
/// started acting, and the width of input its knee spanned (its knee over the slope of the
/// stages below it there). Stages that are off keep theirs.
pub fn on_the_input(old: &[Fields; STAGES]) -> [(f32, f32); STAGES] {
    let v1: [legacy::Stage; STAGES] = std::array::from_fn(|i| {
        let (on, quad, threshold_db, ratio, knee_db, range_db) = old[i];
        legacy::Stage { on, quadrant: Quadrant::from_index(quad), threshold_db, ratio, knee_db, range_db }
    });
    std::array::from_fn(|i| {
        let s = &v1[i];
        if !s.on {
            return (s.threshold_db, s.knee_db);
        }
        let t = legacy::input_level_at_threshold(&v1, i);
        let slope = (legacy::level_before(&v1, i, t + 0.01) - legacy::level_before(&v1, i, t - 0.01)) / 0.02;
        // Nothing below it bends the curve there (slope 1): the knee stays exactly as it was.
        let knee = if (slope - 1.0).abs() < 1e-3 { s.knee_db } else if slope > 1e-3 { s.knee_db / slope } else { S_KNEE.max };
        (t.clamp(S_THRESH.min, S_THRESH.max), knee.clamp(S_KNEE.min, S_KNEE.max))
    })
}

/// Version 1's curve: the stages in series in threshold order, each threshold in the level the
/// stages before it leave.
mod legacy {
    use mite_dsp::dynamics::{soft_above, Quadrant, INF_RATIO, STAGES};

    pub struct Stage {
        pub on: bool,
        pub quadrant: Quadrant,
        pub threshold_db: f32,
        pub ratio: f32,
        pub knee_db: f32,
        pub range_db: f32,
    }

    fn soft_cap(m: f32, range: f32) -> f32 {
        m - soft_above(m - range.max(0.0), 3.0)
    }

    pub fn gain(s: &Stage, level: f32) -> f32 {
        if !s.on {
            return 0.0;
        }
        let over = level - s.threshold_db;
        let r = s.ratio.max(1.0);
        let comp = if r >= INF_RATIO { 1.0 } else { 1.0 - 1.0 / r };
        let expand = r - 1.0;
        match s.quadrant {
            Quadrant::Compress => -soft_cap(comp * soft_above(over, s.knee_db), s.range_db),
            Quadrant::Expand => -soft_cap(expand * soft_above(-over, s.knee_db), s.range_db),
            Quadrant::UpCompress => soft_cap(comp * soft_above(-over, s.knee_db), s.range_db),
            Quadrant::UpExpand => soft_cap(expand * soft_above(over, s.knee_db), s.range_db),
        }
    }

    pub fn order(stages: &[Stage; STAGES]) -> [usize; STAGES] {
        let mut idx = [0, 1, 2, 3];
        idx.sort_by(|&a, &b| {
            let (sa, sb) = (&stages[a], &stages[b]);
            sb.on.cmp(&sa.on).then(sa.threshold_db.total_cmp(&sb.threshold_db)).then(a.cmp(&b))
        });
        idx
    }

    pub fn level_before(stages: &[Stage; STAGES], slot: usize, input_db: f32) -> f32 {
        let mut level = input_db;
        for i in order(stages) {
            if i == slot {
                break;
            }
            level += gain(&stages[i], level);
        }
        level
    }

    pub fn input_level_at_threshold(stages: &[Stage; STAGES], slot: usize) -> f32 {
        let target = stages[slot].threshold_db;
        let (mut lo, mut hi) = (-160.0f32, 40.0f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if level_before(stages, slot, mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mite_dsp::dynamics::{curve_db, Stage};

    fn fields(on: bool, q: Quadrant, t: f32, r: f32) -> Fields {
        (on, q as usize, t, r, 6.0, 60.0)
    }

    #[test]
    fn the_lifted_curve_is_version_1s() {
        // 2:1 from −30, then 4:1 from −20 in the compressed level (an input of −10), and an
        // upward compressor below −40.
        let old = [fields(true, Quadrant::Compress, -30.0, 2.0), fields(true, Quadrant::Compress, -20.0, 4.0), fields(true, Quadrant::UpCompress, -40.0, 2.0), fields(false, Quadrant::Compress, -18.0, 4.0)];
        let lifted = on_the_input(&old);
        assert!((lifted[1].0 + 10.0).abs() < 0.01, "{lifted:?}");
        // Behind the 2:1 stage, its 6 dB knee spanned 12 dB of input.
        assert!((lifted[1].1 - 12.0).abs() < 0.01, "{lifted:?}");
        assert_eq!(lifted[3], (-18.0, 6.0));
        let v1: [legacy::Stage; STAGES] = std::array::from_fn(|i| legacy::Stage { on: old[i].0, quadrant: Quadrant::from_index(old[i].1), threshold_db: old[i].2, ratio: old[i].3, knee_db: old[i].4, range_db: old[i].5 });
        let v2: [Stage; STAGES] = std::array::from_fn(|i| Stage { on: old[i].0, quadrant: Quadrant::from_index(old[i].1), threshold_db: lifted[i].0, ratio: old[i].3, knee_db: lifted[i].1, range_db: old[i].5, ..Stage::default() });
        for i in 0..=70 {
            let x = -60.0 + i as f32;
            let mut level = x;
            for k in legacy::order(&v1) {
                level += legacy::gain(&v1[k], level);
            }
            assert!((curve_db(&v2, x, 0.0) - level).abs() < 0.01, "at {x}: {} vs {level}", curve_db(&v2, x, 0.0));
        }
    }

    #[test]
    fn snapshots_are_lifted_too() {
        let n = |spec: &ParamSpec, v: f32| spec.normalize(v);
        let json = format!(r#"{{"active":0,"slots":[null,{{"s1_on":1.0,"s1_thresh":{},"s1_ratio":{},"s2_on":1.0,"s2_thresh":{},"s2_ratio":{}}},null,null]}}"#, n(&S_THRESH, -30.0), n(&S_RATIO, 2.0), n(&S_THRESH, -20.0), n(&S_RATIO, 4.0));
        let out: serde_json::Value = serde_json::from_str(&lift_snapshots(&json).expect("lifted")).expect("JSON");
        let slot = &out["slots"][1];
        let t2 = S_THRESH.denormalize(slot["s2_thresh"].as_f64().expect("threshold") as f32);
        assert!((t2 + 10.0).abs() < 0.05, "{t2}");
        assert!(slot.get("s3_thresh").is_none() && out["slots"][0].is_null());
    }
}
