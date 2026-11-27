//! Renders the Mite Cut editor to a PNG without a host or window, bound to a small document
//! (five named drum tracks, hits on each, the others' bleed under them).
//!
//!     cargo run -p mite_cut --example snapshot -- out.png [scale] [dark|light|hc|cb]

use std::time::Duration;

use mite_ara::testing::{Audio, TestHost};
use mite_cut::CutShared;
use mite_cut::editor::{ACCENT, CutView};
use mite_cut::session::Session;
use mite_ui::render::render_view;
use mite_ui::{MemoryParamHost, Theme, ThemeKind, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-cut.png".into());
    let scale: f64 = args.next().map_or(Ok(2.0), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let fs = 48_000.0;
    let n = (8.0 * fs) as usize;
    // Each drum a decaying tone every so often, heard by every mic (its own loudest).
    let drums = [(55.0, 0.5, 0.0), (190.0, 0.5, 0.25), (130.0, 1.0, 0.125), (90.0, 2.0, 0.375), (7000.0, 0.25, 0.0)];
    let names = ["Kick In", "Snare Top", "Tom 1", "Floor Tom", "Vox"];
    let mut host = TestHost::new(mite_ara::factory(&mite_cut::ara::INFO));
    let mut first = None;
    for (m, name) in names.iter().enumerate() {
        let mut x = vec![0.0f32; n];
        for (d, &(f, every, at)) in drums.iter().enumerate() {
            let gain = if d == m { 0.8 } else { 0.15 };
            let mut t0 = at;
            while t0 < 7.5 {
                let s0 = (t0 * fs) as usize;
                for i in 0..(0.3 * fs) as usize {
                    let t = i as f64 / fs;
                    x[s0 + i] += (gain * (-t / 0.08).exp() * (std::f64::consts::TAU * f * t).sin()) as f32;
                }
                t0 += every;
            }
        }
        let seq = host.add_track(name);
        let src = mite_ara::document::leak(&format!("src-{m}"));
        let md = mite_ara::document::leak(&format!("mod-{m}"));
        let (_s, _md, rg) = host.add_clip_to(seq, Audio { fs, l: x.clone(), r: x }, src, md, 0.0, 0.0, 8.0);
        first.get_or_insert(rg);
    }
    let session = Session::new();
    session.set_sample_rate(fs as f32);
    // SAFETY: the test host's document controller, made by our factory.
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    host.give(inst.extension(), first.expect("a clip"));
    inst.document.wait_reads(Duration::from_secs(20));
    session.bind_ara(inst);
    session.wait_idle(Duration::from_secs(60));

    let host_params = MemoryParamHost::new();
    let mut view = CutView::new(std::sync::Arc::new(CutShared { session }));
    view.tick(&host_params);
    let image = render_view(&mut view, &host_params, &Theme::load(theme, ACCENT), scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
