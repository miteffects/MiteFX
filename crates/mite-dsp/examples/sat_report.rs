//! Mite Sat's aliasing and THD per model and oversampling factor (`mite_dsp::sat::distortion`),
//! as the Markdown tables in `docs/dsp/sat-aliasing.md`.
//!
//!     cargo run --release -p mite-dsp --example sat_report > docs/dsp/sat-aliasing-tables.md

use mite_dsp::sat::{FACTORS, MEASURE_N, Model, distortion};

fn main() {
    let fs = 48_000.0f32;
    let hz = |cycles: usize| f64::from(fs) * cycles as f64 / MEASURE_N as f64;
    // Prime cycle counts, so no harmonic folds back onto another's bin.
    let (thd_tone, mid, high) = (341, 1709, 3413);
    let header = |what: &str| {
        println!(
            "| {what} | {} |",
            FACTORS.map(|f| format!("{f}×")).join(" | ")
        );
        println!("|---|{}", "---:|".repeat(FACTORS.len()));
    };
    for (title, cycles, amp, drive) in [
        (
            "Aliasing, {f} Hz at −6 dBFS, drive 18 dB (dB under the strongest partial)",
            mid,
            0.5f32,
            18.0f32,
        ),
        (
            "Aliasing, {f} Hz at −6 dBFS, drive 18 dB (dB under the strongest partial)",
            high,
            0.5,
            18.0,
        ),
        (
            "Aliasing, {f} Hz at −6 dBFS, drive 36 dB (dB under the strongest partial)",
            high,
            0.5,
            36.0,
        ),
    ] {
        println!(
            "\n### {}\n",
            title.replace("{f}", &format!("{:.0}", hz(cycles)))
        );
        header("Model");
        for m in Model::ALL {
            let row: Vec<String> = FACTORS
                .iter()
                .map(|&f| {
                    format!(
                        "{:.1}",
                        distortion(m, f, fs, cycles, amp, drive, 0.0).alias_db
                    )
                })
                .collect();
            println!("| {} | {} |", m.name(), row.join(" | "));
        }
    }
    println!(
        "\n### THD, {:.0} Hz at −18 dBFS (dB against the fundamental), 2×\n",
        hz(thd_tone)
    );
    println!("| Model | 6 dB | 12 dB | 24 dB | 36 dB |\n|---|---:|---:|---:|---:|");
    for m in Model::ALL {
        let row: Vec<String> = [6.0, 12.0, 24.0, 36.0]
            .iter()
            .map(|&d| {
                format!(
                    "{:.1}",
                    distortion(m, 2, fs, thd_tone, 0.125, d, 0.0).thd_db
                )
            })
            .collect();
        println!("| {} | {} |", m.name(), row.join(" | "));
    }
}
