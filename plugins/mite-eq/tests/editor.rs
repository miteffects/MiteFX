//! Editor gestures for dynamics: the plot grips, the gain dial's ring and the dynamics scope.

use std::sync::Arc;

use mite_core::{ParamDisplay, ParamScale, ParamSpec};
use mite_dsp::eq::BandType;
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, ATTACK, BANDS, DMODE, DYN, SUSTAIN, TRANSIENT, TRANSITION, TTIME, FREQ, GAIN, ON, OUT_GAIN, Q, RANGE, RATIO, RELEASE, SLOPE, STEREO, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::vello::Scene;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, Typesetter, UiEvent, View};

fn index_spec(id: &'static str, count: usize, default: usize) -> ParamSpec {
    ParamSpec {
        id,
        name: id,
        unit: "",
        min: 0.0,
        max: (count - 1) as f32,
        default: default as f32,
        scale: ParamScale::Linear,
        smoothing_ms: 0.0,
        decimals: 0,
        wheel_step: 1.0,
        wheel_step_fine: 1.0,
        display: ParamDisplay::Number,
    }
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// One enabled bell at 1 kHz, 0 dB: its node sits at (690.2, 362) in design units.
fn setup() -> (EqView, MemoryParamHost) {
    let mut host = MemoryParamHost::new()
        .with(OUT_GAIN)
        .with_id("mode", index_spec("mode", 3, 0))
        .with_id("lp_res", index_spec("lp_res", 4, 2));
    for b in 0..BANDS {
        host = host
            .with_id(leak(band_id(b, ON.id)), index_spec("on", 2, 0))
            .with_id(leak(band_id(b, TYPE.id)), index_spec("type", TYPE.choices.len(), 0))
            .with_id(leak(band_id(b, FREQ.id)), FREQ)
            .with_id(leak(band_id(b, GAIN.id)), GAIN)
            .with_id(leak(band_id(b, Q.id)), Q)
            .with_id(leak(band_id(b, SLOPE.id)), index_spec("slope", SLOPE.choices.len(), SLOPE.default))
            .with_id(leak(band_id(b, STEREO.id)), index_spec("stereo", STEREO.choices.len(), 0))
            .with_id(leak(band_id(b, DYN.id)), index_spec("dyn", 2, 0))
            .with_id(leak(band_id(b, RANGE.id)), RANGE)
            .with_id(leak(band_id(b, THRESH.id)), THRESH)
            .with_id(leak(band_id(b, ATTACK.id)), ATTACK)
            .with_id(leak(band_id(b, RELEASE.id)), RELEASE)
            .with_id(leak(band_id(b, RATIO.id)), RATIO)
            .with_id(leak(band_id(b, DMODE.id)), index_spec("dmode", 2, 0))
            .with_id(leak(band_id(b, SUSTAIN.id)), SUSTAIN)
            .with_id(leak(band_id(b, TRANSIENT.id)), TRANSIENT)
            .with_id(leak(band_id(b, TTIME.id)), TTIME)
            .with_id(leak(band_id(b, TRANSITION.id)), TRANSITION);
    }
    host.set_plain(&band_id(0, ON.id), 1.0);
    host.set_plain(&band_id(0, TYPE.id), BandType::Bell as usize as f32);
    host.set_plain(&band_id(0, FREQ.id), 1000.0);
    let mut view = EqView::new(Arc::new(EqShared::new()));
    click(&mut view, &host, NODE);
    draw(&mut view, &host);
    (view, host)
}

const NODE: Point = Point::new(690.2, 362.0);

fn draw(view: &mut EqView, host: &MemoryParamHost) {
    let theme = Theme::load(ThemeKind::Dark, ACCENT);
    let mut text = Typesetter::new();
    let size = view.logical_size();
    view.draw(&mut DrawCx { scene: &mut Scene::new(), theme: &theme, text: &mut text, host, size });
}

fn mods() -> Modifiers {
    Modifiers::default()
}

fn click(view: &mut EqView, host: &MemoryParamHost, at: Point) {
    view.event(&UiEvent::PointerMove { pos: at, mods: mods() }, host);
    view.event(&UiEvent::PointerDown { pos: at, button: MouseButton::Left, mods: mods() }, host);
    view.event(&UiEvent::PointerUp { pos: at, button: MouseButton::Left, mods: mods() }, host);
}

fn drag(view: &mut EqView, host: &MemoryParamHost, from: Point, to: Point) {
    view.event(&UiEvent::PointerMove { pos: from, mods: mods() }, host);
    view.event(&UiEvent::PointerDown { pos: from, button: MouseButton::Left, mods: mods() }, host);
    view.event(&UiEvent::PointerMove { pos: to, mods: mods() }, host);
    view.event(&UiEvent::PointerUp { pos: to, button: MouseButton::Left, mods: mods() }, host);
    draw(view, host);
}

fn plain(host: &MemoryParamHost, param: &str) -> f32 {
    let id = band_id(0, param);
    host.plain(&id, host.normalized(&id))
}

#[test]
fn pulling_a_ghost_grip_makes_the_band_dynamic_and_back_makes_it_static() {
    let (mut view, host) = setup();
    // The grip above the node lifts: pulled 30 px further up, the band lifts by about 4 dB.
    drag(&mut view, &host, Point::new(NODE.x, NODE.y - 28.0), Point::new(NODE.x, NODE.y - 58.0));
    assert_eq!(plain(&host, DYN.id), 1.0);
    let range = plain(&host, RANGE.id);
    assert!((3.5..4.5).contains(&range), "range {range}");
    // The range end is now the grip: pulled below the node, the band ducks instead.
    let end = NODE.y - f64::from(range) * 264.0 / 18.0;
    drag(&mut view, &host, Point::new(NODE.x, end), Point::new(NODE.x, NODE.y + 44.0));
    assert!(plain(&host, RANGE.id) < -2.5);
    assert_eq!(plain(&host, DYN.id), 1.0);
    // Back onto the node: static again.
    let end = NODE.y - f64::from(plain(&host, RANGE.id)) * 264.0 / 18.0;
    drag(&mut view, &host, Point::new(NODE.x, end), NODE);
    assert_eq!(plain(&host, DYN.id), 0.0);
}

#[test]
fn pulling_the_gain_ring_left_ducks_and_right_lifts() {
    let (mut view, host) = setup();
    // The panel is docked bottom centre (x 250, y 562); the gain dial is at (762, 606).
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x - 60.0, ring.y));
    assert_eq!(plain(&host, DYN.id), 1.0);
    assert!(plain(&host, RANGE.id) < -5.0);
    // The gain itself is untouched.
    assert_eq!(plain(&host, GAIN.id), 0.0);
    // Double-clicking the ring makes the band static.
    view.event(&UiEvent::PointerMove { pos: ring, mods: mods() }, &host);
    view.event(&UiEvent::DoubleClick { pos: ring, mods: mods() }, &host);
    assert_eq!(plain(&host, DYN.id), 0.0);
    draw(&mut view, &host);
    // Pulled right from static, it lifts.
    drag(&mut view, &host, ring, Point::new(ring.x + 40.0, ring.y));
    assert_eq!(plain(&host, DYN.id), 1.0);
    assert!(plain(&host, RANGE.id) > 3.0);
}

#[test]
fn the_range_stops_where_the_gain_dial_ends() {
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, GAIN.id), 24.0);
    draw(&mut view, &host);
    // Pulling the ring far right asks for +30 dB; with +24 dB of gain only +6 dB fits.
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x + 400.0, ring.y));
    assert!((plain(&host, RANGE.id) - 6.0).abs() < 0.01, "range {}", plain(&host, RANGE.id));
}

#[test]
fn the_transient_scope_sets_the_attack_amount_and_the_sustain() {
    let (mut view, host) = setup();
    // Make the band dynamic with the ring, then pick the transient shaper beside the scope.
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x + 20.0, ring.y));
    draw(&mut view, &host);
    // The panel is docked at the bottom centre (x 250); with dynamics its top is at y 458 and the
    // dynamics row's scope spans x 264…464, y 470…556. The mode buttons sit at x 476…506.
    click(&mut view, &host, Point::new(491.0, 535.0));
    assert_eq!(plain(&host, DMODE.id), 1.0);
    draw(&mut view, &host);
    let range = plain(&host, RANGE.id);
    drag(&mut view, &host, Point::new(290.0, 520.0), Point::new(290.0, 500.0));
    assert!((plain(&host, TRANSIENT.id) - 3.0).abs() < 0.01, "attack {}", plain(&host, TRANSIENT.id));
    // Its own amount: the compressor's range is untouched.
    assert_eq!(plain(&host, RANGE.id), range);
    drag(&mut view, &host, Point::new(420.0, 520.0), Point::new(420.0, 540.0));
    assert!((plain(&host, SUSTAIN.id) + 3.0).abs() < 0.01, "sustain {}", plain(&host, SUSTAIN.id));
    // The handover line sits at 25 ms on the square-root time axis (x ≈ 313); dragged to the
    // spot for 100 ms, the transient time follows.
    assert_eq!(plain(&host, TTIME.id), 25.0);
    let at = |ms: f64| 272.0 + ((ms / 1000.0 + 0.005) / 0.6).sqrt() * 184.0;
    drag(&mut view, &host, Point::new(at(25.0), 520.0), Point::new(at(100.0), 520.0));
    assert!((plain(&host, TTIME.id) - 100.0).abs() < 0.5, "time {}", plain(&host, TTIME.id));
}

#[test]
fn the_footer_picks_the_processing_mode_and_resolution() {
    let (mut view, host) = setup();
    let mode = |host: &MemoryParamHost| host.plain("mode", host.normalized("mode"));
    click(&mut view, &host, Point::new(193.0, 732.0));
    assert_eq!(mode(&host), 1.0);
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(314.0, 732.0));
    assert_eq!(mode(&host), 2.0);
    draw(&mut view, &host);
    // The resolution dropdown opens upwards from the footer; Max is its last item.
    click(&mut view, &host, Point::new(444.0, 732.0));
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(444.0, 691.0));
    assert_eq!(host.plain("lp_res", host.normalized("lp_res")), 3.0);
}
