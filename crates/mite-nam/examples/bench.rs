//! Real-time cost of a model: `cargo run --release -p mite-nam --example bench -- model.nam [block]`
//! prints the share of one core it takes at its own rate.

use std::time::Instant;

use mite_nam::{LoadOptions, NamModel};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args.get(1).expect("a .nam file");
    let block: usize = args.get(2).and_then(|b| b.parse().ok()).unwrap_or(128);
    let size: f64 = args.get(3).and_then(|b| b.parse().ok()).unwrap_or(1.0);
    let mut m = NamModel::from_file(path.as_ref(), LoadOptions { max_block: block, size, ..LoadOptions::default() }).expect("loads");
    let rate = m.info().sample_rate.unwrap_or(48_000.0);
    let seconds = 10.0;
    let n = (rate * seconds) as usize;
    let x: Vec<f32> = (0..n).map(|i| 0.3 * ((i as f32) * 0.0123).sin()).collect();
    let mut y = vec![0.0; n];
    let t = Instant::now();
    for (xs, ys) in x.chunks(block).zip(y.chunks_mut(block)) {
        m.process(xs, ys);
    }
    let took = t.elapsed().as_secs_f64();
    println!("{path}: {:.2} % of a core (block {block}, size {size})", 100.0 * took / seconds);
}
