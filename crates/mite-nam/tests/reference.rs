//! Sample-by-sample agreement with NeuralAmpModelerCore (the reference implementation) on its
//! example models (tests/models, MIT, Steven Atkinson). The golden outputs were rendered by the
//! reference at 64-sample blocks, after its pre-warm, for the input below.
//!
//! To regenerate: build NeuralAmpModelerCore with a harness that loads the model, optionally
//! enables fast tanh, calls `ResetAndPrewarm(rate, 64)`, optionally `SetSlimmableSize`, and
//! writes `process` of `input_signal(4096)` in 64-sample blocks as raw little-endian f32.

use std::path::PathBuf;

use mite_nam::{Generation, LoadOptions, NamModel};

const N: usize = 4096;

/// The reference harness's input: LCG noise in 16 steps of level (exact in f32 on both sides).
fn input_signal(n: usize) -> Vec<f32> {
    let mut s: u32 = 12345;
    (0..n)
        .map(|i| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let u = (s >> 9) as f32 / 8_388_608.0 * 2.0 - 1.0;
            let a = (((i / 512) % 16) + 1) as f32 / 16.0;
            a * u
        })
        .collect()
}

fn dir(sub: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(sub)
}

fn golden(name: &str) -> Vec<f32> {
    let bytes = std::fs::read(dir("golden").join(format!("{name}.f32"))).expect("golden file");
    bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn render(model: &str, options: LoadOptions, block: usize) -> (Vec<f32>, NamModel) {
    let mut m = NamModel::from_file(&dir("models").join(format!("{model}.nam")), options).unwrap_or_else(|e| panic!("{model}: {e}"));
    let x = input_signal(N);
    let mut y = vec![0.0; N];
    for (xs, ys) in x.chunks(block).zip(y.chunks_mut(block)) {
        m.process(xs, ys);
    }
    (y, m)
}

/// The largest error relative to the reference's peak.
fn error(ours: &[f32], reference: &[f32]) -> f32 {
    let peak = reference.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-9);
    ours.iter().zip(reference).fold(0.0f32, |m, (a, b)| m.max((a - b).abs())) / peak
}

#[test]
fn every_example_model_matches_the_reference() {
    let opts = |fast: bool, size: f64| LoadOptions { fast_tanh: fast, size, max_block: 64 };
    let cases: &[(&str, &str, bool, f64)] = &[
        ("wavenet", "wavenet", true, 1.0),
        ("wavenet", "wavenet.exact_tanh", false, 1.0),
        ("lstm", "lstm", true, 1.0),
        ("lstm", "lstm.exact_tanh", false, 1.0),
        ("wavenet_a1_standard", "wavenet_a1_standard", true, 1.0),
        ("wavenet_a2_feature_test", "wavenet_a2_feature_test", true, 1.0),
        ("wavenet_condition_dsp", "wavenet_condition_dsp", true, 1.0),
        ("slimmable_wavenet", "slimmable_wavenet", true, 1.0),
        ("slimmable_container", "slimmable_container", true, 1.0),
        ("slimmable_container", "slimmable_container.size0.5", true, 0.5),
        ("slimmable_container", "slimmable_container.size0.2", true, 0.2),
        ("A2", "A2", true, 1.0),
        ("A2", "A2.size0.3", true, 0.3),
    ];
    let mut failures = Vec::new();
    for &(model, gold, fast, size) in cases {
        let reference = golden(gold);
        let (ours, _) = render(model, opts(fast, size), 64);
        let e = error(&ours, &reference);
        println!("{gold:34} error {e:.2e} of peak");
        if e.is_nan() || e >= 2e-5 {
            failures.push(format!("{gold}: {e:.2e}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[test]
fn the_output_does_not_depend_on_the_block_size() {
    let (a, _) = render("A2", LoadOptions { max_block: 64, ..LoadOptions::default() }, 64);
    let (b, _) = render("A2", LoadOptions { max_block: 512, ..LoadOptions::default() }, 37);
    assert!(error(&b, &a) < 1e-6, "{}", error(&b, &a));
}

#[test]
fn a2_files_say_what_they_are() {
    let (_, m) = render("A2", LoadOptions::default(), 512);
    let info = m.info();
    assert_eq!(info.generation, Generation::A2);
    assert_eq!(info.architecture, "WaveNet");
    assert_eq!(info.sample_rate, Some(48000.0));
    assert!((info.loudness.unwrap() + 26.3839).abs() < 1e-3);
    assert_eq!(info.input_level_dbu, Some(14.76));
    assert_eq!(info.sizes, vec![0.5, 1.0]);
    assert_eq!(info.size_index, 1);
    let (_, lite) = render("A2", LoadOptions { size: 0.2, ..LoadOptions::default() }, 512);
    assert_eq!(lite.info().size_index, 0);
    let (_, a1) = render("wavenet_a1_standard", LoadOptions::default(), 512);
    assert_eq!(a1.info().generation, Generation::A1);
}

#[test]
fn broken_files_are_refused_with_a_reason() {
    let refuse = |text: &str| NamModel::from_json(text, LoadOptions::default()).err().expect("refused");
    assert!(refuse("not json").contains("not a NAM model"));
    assert!(refuse(r#"{"version":"0.9.0","architecture":"WaveNet","config":{},"weights":[]}"#).contains("newer"));
    assert!(refuse(r#"{"version":"0.5.4","architecture":"Transformer","config":{},"weights":[]}"#).contains("not supported"));
    let mut wavenet: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir("models").join("wavenet.nam")).unwrap()).unwrap();
    wavenet["weights"].as_array_mut().unwrap().pop();
    wavenet["weights"].as_array_mut().unwrap().pop();
    assert!(refuse(&wavenet.to_string()).contains("fewer weights"));
}
