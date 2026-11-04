//! Times the editor while a tap, its feedback and a loop band are dragged (four taps and four
//! loop bands on, Tape): the worst event and the worst draw.
//!
//!     cargo run --release -p mite_delay --example drag_bench

use std::sync::Arc;
use std::time::Instant;

use mite_delay::editor::{memory_host, DelayView, Node, ACCENT};
use mite_delay::DelayShared;
use mite_ui::kurbo::Point;
use mite_ui::{DrawCx, Modifiers, MouseButton, Theme, ThemeKind, UiEvent, View};

fn main() {
    let host = memory_host();
    host.set_plain("mode", 1.0);
    for t in 1..4 {
        host.set_plain(&format!("t{}_on", t + 1), 1.0);
        host.set_plain(&format!("t{}_note", t + 1), [5.0, 8.0, 13.0][t - 1]);
        host.set_plain(&format!("t{}_feedback", t + 1), 70.0);
    }
    for (b, f, kind) in [(0, 200.0, 3.0), (1, 2000.0, 0.0), (2, 600.0, 0.0), (3, 6000.0, 4.0)] {
        for (id, v) in [("on", 1.0), ("type", kind), ("freq", f), ("gain", -3.0)] {
            host.set_plain(&format!("e{}_{id}", b + 1), v);
        }
    }
    let mut view = DelayView::new(Arc::new(DelayShared::new()));
    let theme = Theme::load(ThemeKind::Dark, ACCENT);
    let mut text = mite_ui::Typesetter::new();
    let mut draw = |view: &mut DelayView| {
        let t = Instant::now();
        view.draw(&mut DrawCx { scene: &mut mite_ui::vello::Scene::new(), theme: &theme, text: &mut text, host: &host, size: view.logical_size(), fx: &mut Default::default() });
        t.elapsed().as_secs_f64() * 1000.0
    };
    draw(&mut view);
    let mods = Modifiers::default();
    for node in [Node::Tap(2), Node::Feedback(2), Node::Band(1)] {
        let p = view.node(&host, node);
        view.event(&UiEvent::PointerMove { pos: p, mods }, &host);
        view.event(&UiEvent::PointerDown { pos: p, button: MouseButton::Left, mods }, &host);
        let (mut ev, mut dr) = (0.0f64, 0.0f64);
        for k in 1..=30 {
            let t = Instant::now();
            view.event(&UiEvent::PointerMove { pos: Point::new(p.x + k as f64 * 3.0, p.y + k as f64 * 1.5), mods }, &host);
            ev = ev.max(t.elapsed().as_secs_f64() * 1000.0);
            dr = dr.max(draw(&mut view));
        }
        view.event(&UiEvent::PointerUp { pos: p, button: MouseButton::Left, mods }, &host);
        println!("{node:?}: worst event {ev:.2} ms, worst draw {dr:.2} ms");
    }
}
