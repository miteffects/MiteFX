//! Times the editor while a decay band is dragged (four bands and two steep cuts on): the worst event and the worst
//! draw. The exact 32-line decay mean cost 16 ms an event and froze the UI (2026-11-04); the
//! decay model brought it under a millisecond.
//!
//!     cargo run --release -p mite_verb --example drag_bench

use std::sync::Arc;
use std::time::Instant;
use mite_ui::kurbo::Point;
use mite_ui::{Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View, DrawCx};
use mite_verb::editor::{memory_host, Node, VerbView, ACCENT};
use mite_verb::VerbShared;

fn main() {
    let host = memory_host();
    for (b, f, t) in [(0, 200.0, 1.0), (1, 2000.0, 2.0), (2, 600.0, 2.0), (3, 6000.0, 1.0)] {
        for (id, v) in [("on", 1.0), ("type", t), ("freq", f), ("mult", 0.6)] {
            host.set_plain(&format!("d{}_{id}", b + 1), v);
        }
    }
    // And two cuts at the steepest slope (eight sections each in every line).
    for (b, f, t) in [(4, 120.0, 4.0), (5, 9000.0, 5.0)] {
        for (id, v) in [("on", 1.0), ("type", t), ("freq", f), ("slope", 8.0)] {
            host.set_plain(&format!("d{}_{id}", b + 1), v);
        }
    }
    let mut view = VerbView::new(Arc::new(VerbShared::new()));
    let theme = Theme::load(ThemeKind::Dark, ACCENT);
    let mut text = mite_ui::Typesetter::new();
    let mut draw = |view: &mut VerbView| {
        let t = Instant::now();
        view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host: &host, size: view.logical_size(), fx: &mut Default::default() });
        t.elapsed().as_secs_f64() * 1000.0
    };
    draw(&mut view);
    let mods = Modifiers::default();
    let p = view.node(&host, Node::Decay(2));
    let t = Instant::now();
    view.event(&UiEvent::PointerMove { pos: p, mods }, &host);
    println!("hover {:.2} ms", t.elapsed().as_secs_f64() * 1000.0);
    view.event(&UiEvent::PointerDown { pos: p, button: MouseButton::Left, mods }, &host);
    let (mut ev, mut dr) = (0.0f64, 0.0f64);
    for k in 1..=30 {
        let t = Instant::now();
        view.event(&UiEvent::PointerMove { pos: Point::new(p.x + k as f64 * 3.0, p.y - k as f64 * 2.0), mods }, &host);
        ev = ev.max(t.elapsed().as_secs_f64() * 1000.0);
        dr = dr.max(draw(&mut view));
    }
    println!("drag move: worst event {ev:.2} ms, worst draw {dr:.2} ms");
}
