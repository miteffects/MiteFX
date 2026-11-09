//! Renders the Mite Pitch editor to a PNG with a sung take transferred through the real engine,
//! its second note dragged up a tone.
//!
//!     cargo run -p mite_pitch --example snapshot -- out.png [scale] [dark|light|hc|cb]
//!
//! SNAPSHOT_EMPTY=1: no take. SNAPSHOT_ARA=1: bound to an ARA document (a Rust host) with two
//! clips on the track, the second being edited, the first faint beside it.

#[path = "common/take.rs"]
mod common;

use std::sync::Arc;

use mite_dsp::melody::NoteEdit;
use mite_pitch::PitchShared;
use mite_pitch::dsp::PitchEngine;
use mite_pitch::editor::{ACCENT, PitchView, memory_host};
use mite_ui::render::render_view;
use mite_ui::{Theme, ThemeKind, View};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "mite-pitch.png".into());
    let scale: f64 = args.next().map_or(Ok(1.5), |s| s.parse())?;
    let theme = match args.next().as_deref() {
        Some("light") => ThemeKind::Light,
        Some("hc") => ThemeKind::HighContrast,
        Some("cb") => ThemeKind::ColorBlind,
        _ => ThemeKind::Dark,
    };
    let host = memory_host();
    let s = common::session("snapshot");
    // The ARA host lives as long as the picture is being made.
    let mut _ara_host = None;
    if std::env::var("SNAPSHOT_ARA").is_ok() {
        use mite_ara::testing::{Audio, TestHost};
        use mite_dsp::melody::voice::{sing, Sung};
        let mut host = TestHost::new(mite_ara::factory(&mite_pitch::ara::INFO));
        let a = common::take();
        let n = |cents: f64, seconds: f64| Sung { cents, seconds, vibrato_cents: 25.0, vibrato_hz: 5.0, db: -14.0 };
        let b = sing(&[n(6230.0, 0.45), n(6010.0, 0.35), n(5790.0, 0.7), n(5700.0, 0.5)], 0.08, common::FS).0;
        let (da, db) = (a.len() as f64 / common::FS, b.len() as f64 / common::FS);
        host.add_clip(Audio { fs: common::FS, l: a.clone(), r: a }, c"src-a", c"mod-a", 0.5, 0.0, da);
        let (_, _, rb) = host.add_clip(Audio { fs: common::FS, l: b.clone(), r: b }, c"src-b", c"mod-b", 0.5 + da + 0.4, 0.0, db);
        // SAFETY: the host's document controller, made by our factory.
        let inst = unsafe { mite_ara::bind(host.dc, 7, 6) };
        host.show_track(inst.extension());
        inst.document.wait_reads(std::time::Duration::from_secs(10));
        s.bind_ara(inst);
        s.wait_idle(std::time::Duration::from_secs(30));
        s.pick_clip(mite_ara::document::region_id(rb));
        s.wait_idle(std::time::Duration::from_secs(30));
        s.set_edit(1, NoteEdit { shift_cents: -100.0, ..NoteEdit::default() });
        s.wait_idle(std::time::Duration::from_secs(30));
        _ara_host = Some(host);
    } else if std::env::var("SNAPSHOT_EMPTY").is_err() {
        let mut e = PitchEngine::new(common::FS as f32);
        common::transfer(&mut e, &s, &common::take(), 0);
        s.set_edit(
            1,
            NoteEdit {
                shift_cents: 200.0 - 25.0,
                ..NoteEdit::default()
            },
        );
        s.set_edit(2, NoteEdit { modulation: 0.3, formant_semitones: 4.0, gain_db: -6.0, ..NoteEdit::default() });
        s.wait_idle(std::time::Duration::from_secs(30));
        common::play(&mut e, &s, &vec![0.0; 40_000], 0, 512);
    }
    let shared = Arc::new(PitchShared {
        session: s,
        sample_rate: mite_core::AtomicF32::new(common::FS as f32),
    });
    let mut view = PitchView::new(shared);
    view.tick(&host);
    let theme_obj = Theme::load(theme, ACCENT);
    render_view(&mut view, &host, &theme_obj, 1.0)?;
    if _ara_host.is_some() {
        // Zoomed out over both clips, the pointer over the faint one.
        let mods = mite_ui::Modifiers { shift: false, alt: false, ctrl: true, meta: false };
        view.event(&mite_ui::UiEvent::Wheel { pos: mite_ui::kurbo::Point::new(1100.0, 300.0), lines: -6.0, mods }, &host);
        view.event(&mite_ui::UiEvent::PointerMove { pos: mite_ui::kurbo::Point::new(view.x_of(-1.5), 300.0), mods: mite_ui::Modifiers::default() }, &host);
    }
    let image = render_view(&mut view, &host, &theme_obj, scale)?;
    let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
    let mut encoder = png::Encoder::new(file, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&image.rgba)?;
    println!("wrote {path} ({}×{})", image.width, image.height);
    Ok(())
}
