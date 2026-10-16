//! Editor gestures for dynamics: the plot grips, the gain dial's ring and the dynamics scope.

use std::sync::Arc;

use mite_dsp::eq::BandType;
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, DMODE, DYN, SUSTAIN, TRANSIENT, TTIME, SPECTRAL, SSMOOTH, SREL, RTHRESH, FREQ, GAIN, ON, RANGE, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::vello::Scene;
use mite_ui::{DrawCx, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, Typesetter, UiEvent, View};

/// One enabled bell at 1 kHz, 0 dB: its node sits at (690.2, 362) in design units.
fn setup() -> (EqView, MemoryParamHost) {
    setup_with(Arc::new(EqShared::new()))
}

fn setup_with(shared: Arc<EqShared>) -> (EqView, MemoryParamHost) {
    let host = mite_eq::editor::memory_host();
    host.set_plain(&band_id(0, ON.id), 1.0);
    host.set_plain(&band_id(0, TYPE.id), BandType::Bell as usize as f32);
    host.set_plain(&band_id(0, FREQ.id), 1000.0);
    let mut view = EqView::new(shared);
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

#[test]
fn the_middle_mode_button_makes_the_compressor_spectral() {
    let (mut view, host) = setup();
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x - 40.0, ring.y));
    draw(&mut view, &host);
    // Three mode buttons beside the scope (x 476…506): compressor 470…496, spectral 500…526,
    // transient shaper 530…556.
    click(&mut view, &host, Point::new(491.0, 513.0));
    assert_eq!(plain(&host, SPECTRAL.id), 1.0);
    assert_eq!(plain(&host, DMODE.id), 0.0);
    draw(&mut view, &host);
    // The transient shaper is not spectral; back to the plain compressor turns spectral off.
    click(&mut view, &host, Point::new(491.0, 543.0));
    assert_eq!((plain(&host, DMODE.id), plain(&host, SPECTRAL.id)), (1.0, 0.0));
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(491.0, 513.0));
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(491.0, 483.0));
    assert_eq!((plain(&host, DMODE.id), plain(&host, SPECTRAL.id)), (0.0, 0.0));
}

#[test]
fn the_spectral_scope_drags_the_threshold_and_adds_smoothing() {
    let (mut view, host) = setup();
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x - 40.0, ring.y));
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(491.0, 513.0));
    draw(&mut view, &host);
    // The spectral scope's threshold line follows the pointer up: 14 px is 12 dB of its 60 dB
    // over 70 px.
    let (thresh, range) = (plain(&host, THRESH.id), plain(&host, RANGE.id));
    drag(&mut view, &host, Point::new(330.0, 520.0), Point::new(330.0, 506.0));
    assert!((plain(&host, THRESH.id) - thresh - 12.0).abs() < 0.01, "threshold {}", plain(&host, THRESH.id));
    // The range is the ring's; this scope leaves it alone.
    assert_eq!(plain(&host, RANGE.id), range);
    // A fifth dial, SMOOTHING, sits last in the row (x 308 + 4 × 70 from the panel's left at 250).
    let at = Point::new(250.0 + 308.0 + 280.0, 458.0 + 38.0);
    let before = plain(&host, SSMOOTH.id);
    drag(&mut view, &host, at, Point::new(at.x, at.y - 40.0));
    assert!(plain(&host, SSMOOTH.id) > before, "smoothing {} → {}", before, plain(&host, SSMOOTH.id));
}

#[test]
fn the_relative_toggle_makes_the_threshold_follow_the_surroundings() {
    let (mut view, host) = setup();
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x - 40.0, ring.y));
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(491.0, 513.0));
    draw(&mut view, &host);
    // Spectral bands get a toggle under the remove-dynamics button (x 906…936, y 520…550).
    click(&mut view, &host, Point::new(921.0, 535.0));
    assert_eq!(plain(&host, SREL.id), 1.0);
    draw(&mut view, &host);
    // Now the scope's line is the relative threshold (dB over the surroundings): 14 px up is
    // 12 dB more, and the fixed threshold stays where it was.
    let (relative, fixed) = (plain(&host, RTHRESH.id), plain(&host, THRESH.id));
    drag(&mut view, &host, Point::new(330.0, 520.0), Point::new(330.0, 506.0));
    assert!((plain(&host, RTHRESH.id) - relative - 12.0).abs() < 0.01, "relative threshold {}", plain(&host, RTHRESH.id));
    assert_eq!(plain(&host, THRESH.id), fixed);
    // And back to a fixed level.
    click(&mut view, &host, Point::new(921.0, 535.0));
    assert_eq!(plain(&host, SREL.id), 0.0);
}

#[test]
fn live_spectral_cuts_draw_on_the_plot() {
    let shared = Arc::new(EqShared::new());
    let (mut view, host) = setup_with(shared.clone());
    host.set_plain(&band_id(0, DYN.id), 1.0);
    host.set_plain(&band_id(0, SPECTRAL.id), 1.0);
    shared.dynamic[0].store(-8.0);
    for slot in &shared.focus_level {
        slot.store(-30.0);
    }
    for (i, slot) in shared.spectral.iter().enumerate() {
        slot.store(if (120..136).contains(&i) { -8.0 } else { 0.0 });
    }
    // The telemetry change asks for a frame, and the frame draws with it.
    assert!(view.tick(&host));
    draw(&mut view, &host);
}

fn global(host: &MemoryParamHost, id: &str) -> f32 {
    host.plain(id, host.normalized(id))
}

#[test]
fn the_footer_toggles_auto_gain_delta_and_bypass() {
    let (mut view, host) = setup();
    // Three icon toggles left of the dials (x 852…966).
    for (x, id) in [(869.0, "auto_gain"), (909.0, "delta"), (949.0, "bypass")] {
        click(&mut view, &host, Point::new(x, 732.0));
        assert_eq!(global(&host, id), 1.0, "{id} on");
        draw(&mut view, &host);
        click(&mut view, &host, Point::new(x, 732.0));
        assert_eq!(global(&host, id), 0.0, "{id} off");
        draw(&mut view, &host);
    }
}

#[test]
fn the_footer_mix_dial_drags_down_to_dry() {
    let (mut view, host) = setup();
    // Dials at x 1012 (mix), 1104 (pan), 1196 (output).
    drag(&mut view, &host, Point::new(1012.0, 732.0), Point::new(1012.0, 832.0));
    let mix = global(&host, "mix");
    assert!(mix < 60.0, "mix {mix}");
}

#[test]
fn solo_listens_to_one_band_until_its_panel_closes() {
    let (mut view, host) = setup();
    // The solo button sits right of the stereo dropdown in the selected band's panel.
    let solo = Point::new(564.0, 632.0);
    click(&mut view, &host, solo);
    assert_eq!(plain(&host, "solo"), 1.0);
    draw(&mut view, &host);
    click(&mut view, &host, solo);
    assert_eq!(plain(&host, "solo"), 0.0);
    draw(&mut view, &host);
    click(&mut view, &host, solo);
    draw(&mut view, &host);
    // Closing the panel (clicking empty plot) ends the solo.
    click(&mut view, &host, Point::new(200.0, 200.0));
    view.tick(&host);
    assert_eq!(plain(&host, "solo"), 0.0);
}

