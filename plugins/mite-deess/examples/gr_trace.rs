//! Prints the reduction, the trigger band and the voice every 10 ms over two syllables of the test
//! voice (to see the reduction follow the sibilants and let go for the vowels).
//!
//!     cargo run --release -p mite_deess --example gr_trace

#[path = "common/voice.rs"]
mod voice;

use mite_dsp::deess::{DeEssSettings, DeEsser};

fn main() {
    let fs = 48_000.0;
    let x = voice::voice(1.2, fs);
    let s = DeEssSettings::default();
    let mut d = DeEsser::new(fs as f32, &s);
    let mut y = x.clone();
    for (k, c) in y.chunks_mut(480).enumerate() {
        let t = d.process(&mut [c], &s);
        if (40..120).contains(&k) && k % 2 == 0 {
            println!("{:4} ms  gr {:5.1}  band {:6.1}  whole {:6.1}  rel {:6.1}", k * 10, t.gr_db, t.band_db, t.whole_db, t.band_db - t.whole_db);
        }
    }
}
