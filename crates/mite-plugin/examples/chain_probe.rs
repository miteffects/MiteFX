//! Runs a chain of installed CLAP plugins offline, as a host would: each loaded from its bundle
//! with a saved state (JSON, e.g. decoded from a REAPER project), fed a mono f32 file. Prints each
//! plugin's reported latency and, per stage and for the whole chain, the delay and polarity
//! found by GCC-PHAT (what the Analyzer's phase page reads). Writes each stage's output next to
//! the input as `<input>.<n>.f32`.
//!
//! `cargo run --profile fasttest -p mite-plugin --example chain_probe -- <fs> <audio.f32> <bundle.clap> <clap id> <state.json> [...]`

use mite_dsp::transfer::{DelayEstimate, DelayFinder};
use mite_plugin::testing::ClapInstance;

fn delay(x: &[f32], y: &[f32], fs: f64) -> Option<DelayEstimate> {
    let n = 1 << 15;
    let mut f = DelayFinder::new(n, fs);
    let mut last = None;
    let mut end = n;
    while end <= x.len().min(y.len()) {
        if let Some(e) = f.update(&[&x[end - n..end]], &[&y[end - n..end]], 0.1) {
            last = Some(e);
        }
        end += n / 4;
    }
    last
}

fn show(d: Option<DelayEstimate>, fs: f64) -> String {
    d.map_or("none".into(), |d| format!("{:.1} samples ({:.2} ms){} conf {:.2}", d.samples, d.samples / fs * 1e3, if d.inverted { " INVERTED" } else { "" }, d.confidence))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fs: f64 = args[0].parse().expect("fs");
    let bytes = std::fs::read(&args[1]).expect("audio");
    let input: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().expect("4 bytes"))).collect();
    let mut libs = Vec::new();
    let mut chain = Vec::new();
    for t in args[2..].chunks(3) {
        let dir = std::path::Path::new(&t[0]);
        let so = if dir.is_dir() { std::fs::read_dir(dir).expect("bundle").flatten().map(|e| e.path()).find(|p| p.is_file()).expect("library") } else { dir.to_path_buf() };
        // SAFETY: a CLAP bundle built from this workspace; the library outlives its plugin.
        let lib = unsafe { libloading::Library::new(&so) }.expect("load bundle");
        // SAFETY: `clap_entry` is the CLAP entry point, a static in the library.
        let entry: &'static clap_sys::entry::clap_plugin_entry = unsafe { &**lib.get::<*const clap_sys::entry::clap_plugin_entry>(b"clap_entry\0").expect("clap_entry") };
        let mut p = ClapInstance::new(entry, &t[1]);
        let state = std::fs::read(&t[2]).expect("state");
        assert!(p.load(&state), "state load failed for {}", t[1]);
        p.activate(fs, 512);
        chain.push(p);
        libs.push(lib);
    }
    let mut stages = vec![input.clone()];
    let mut cur = input;
    for p in &mut chain {
        let mut out = Vec::with_capacity(cur.len());
        for block in cur.chunks(512) {
            let mut b = vec![block.to_vec(), block.to_vec()];
            p.process(&mut b, &[]);
            out.extend_from_slice(&b[0]);
        }
        cur = out;
        stages.push(cur.clone());
    }
    for (i, s) in stages.iter().enumerate().skip(1) {
        let bytes: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).collect();
        std::fs::write(format!("{}.{i}.f32", args[1]), bytes).expect("write stage");
    }
    let mut total = 0;
    for (i, p) in chain.iter().enumerate() {
        let lat = p.latency();
        total += lat;
        let peak = stages[i + 1].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!("{:<12} reports {:>5} samples ({:>6.2} ms); measured {}; out peak {:.3}", p.name(), lat, f64::from(lat) / fs * 1e3, show(delay(&stages[i], &stages[i + 1], fs), fs), peak);
    }
    println!("chain: reports {total} samples ({:.2} ms); measured {}", f64::from(total) / fs * 1e3, show(delay(&stages[0], stages.last().expect("stages"), fs), fs));
    drop(chain);
}
