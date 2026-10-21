//! Editor gestures for dynamics: the plot grips, the gain dial's ring and the dynamics scope.

use std::sync::Arc;

use mite_dsp::eq::BandType;
use mite_eq::editor::{EqView, ACCENT};
use mite_eq::params::{band_id, DMODE, DYN, SUSTAIN, TRANSIENT, TTIME, SPECTRAL, SSMOOTH, SREL, RTHRESH, FREQ, GAIN, ON, Q, RANGE, THRESH, TYPE};
use mite_eq::EqShared;
use mite_ui::kurbo::Point;
use mite_ui::vello::Scene;
use mite_ui::{DrawCx, Key, MemoryParamHost, Modifiers, MouseButton, ParamHost, Theme, ThemeKind, Typesetter, UiEvent, View};

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


/// A view whose spectrum shows a 1 kHz tone at −6 dBFS (about −17.6 dB on the analyzer's scale;
/// the drawn line, through the plot's grid points either side, tops out near y 202), with the
/// bell from `setup` deselected.
fn with_a_peak() -> (EqView, MemoryParamHost) {
    let shared = Arc::new(EqShared::new());
    let tone: Vec<f32> = (0..mite_eq::editor::ANALYZER_SIZE).map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin() * 0.5).collect();
    let (mut view, host) = setup_with(shared.clone());
    // Rest under the tone's peak (inside the spectrum), clear of the bell's node.
    click(&mut view, &host, REST);
    // The analyzer runs at most every 15.5 ms and eases in; give it time to settle.
    for _ in 0..40 {
        shared.pre.push(tone.iter().copied());
        shared.post.push(tone.iter().copied());
        std::thread::sleep(std::time::Duration::from_millis(16));
        view.tick(&host);
    }
    draw(&mut view, &host);
    (view, host)
}

/// Just under the tip of the tone's peak.
const PEAK: Point = Point::new(690.2, 205.0);
const REST: Point = Point::new(690.2, 430.0);

fn band_plain(host: &MemoryParamHost, b: usize, param: &str) -> f32 {
    let id = band_id(b, param);
    host.plain(&id, host.normalized(&id))
}

#[test]
fn dragging_a_spectrum_peak_shapes_a_new_bell_on_it() {
    let (mut view, host) = with_a_peak();
    // Sideways movement is ignored: the bell stays on the peak; only the gain follows.
    drag(&mut view, &host, PEAK, Point::new(PEAK.x + 50.0, PEAK.y + 60.0));
    assert_eq!(band_plain(&host, 1, ON.id), 1.0, "a band was added");
    let freq = band_plain(&host, 1, FREQ.id);
    assert!((freq / 1000.0 - 1.0).abs() < 0.02, "freq {freq}");
    let gain = band_plain(&host, 1, GAIN.id);
    assert!((gain + 60.0 * 18.0 / 264.0).abs() < 0.1, "gain {gain}");
    assert!((band_plain(&host, 1, Q.id) - 3.0).abs() < 0.01);
}

#[test]
fn resting_over_the_plot_holds_the_peaks_after_the_sound_has_gone() {
    // The pointer rests under the spectrum while the tone plays: peak hold
    // collects it. The tone then stops and the live spectrum falls away, but the held peak can
    // still be grabbed.
    let shared = Arc::new(EqShared::new());
    let tone: Vec<f32> = (0..mite_eq::editor::ANALYZER_SIZE).map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / 48_000.0).sin() * 0.5).collect();
    let (mut view, host) = setup_with(shared.clone());
    // Rest under the tone's peak (inside the spectrum), clear of the bell's node.
    click(&mut view, &host, REST);
    for i in 0..100 {
        let block: Vec<f32> = if i < 40 { tone.clone() } else { vec![0.0; tone.len()] };
        shared.pre.push(block.iter().copied());
        shared.post.push(block.iter().copied());
        std::thread::sleep(std::time::Duration::from_millis(16));
        view.tick(&host);
    }
    draw(&mut view, &host);
    drag(&mut view, &host, PEAK, Point::new(PEAK.x, PEAK.y + 30.0));
    assert_eq!(band_plain(&host, 1, ON.id), 1.0, "the held peak was grabbed");
    assert!((band_plain(&host, 1, FREQ.id) / 1000.0 - 1.0).abs() < 0.02);
}

#[test]
fn escape_drops_a_grabbed_peak() {
    let (mut view, host) = with_a_peak();
    view.event(&UiEvent::PointerMove { pos: PEAK, mods: mods() }, &host);
    view.event(&UiEvent::PointerDown { pos: PEAK, button: MouseButton::Left, mods: mods() }, &host);
    view.event(&UiEvent::PointerMove { pos: Point::new(PEAK.x, PEAK.y + 80.0), mods: mods() }, &host);
    assert!(view.event(&UiEvent::KeyDown { key: Key::Escape, mods: mods() }, &host));
    view.event(&UiEvent::PointerUp { pos: Point::new(PEAK.x, PEAK.y + 80.0), button: MouseButton::Left, mods: mods() }, &host);
    assert_eq!(band_plain(&host, 1, ON.id), 0.0, "nothing added");
    // Other keys are the host's.
    assert!(!view.event(&UiEvent::KeyDown { key: Key::Character(' '), mods: mods() }, &host));
}

#[test]
fn double_clicking_a_peak_cuts_it_with_a_narrow_bell() {
    let (mut view, host) = with_a_peak();
    view.event(&UiEvent::PointerMove { pos: PEAK, mods: mods() }, &host);
    view.event(&UiEvent::DoubleClick { pos: PEAK, mods: mods() }, &host);
    assert_eq!(band_plain(&host, 1, ON.id), 1.0);
    assert!((band_plain(&host, 1, FREQ.id) / 1000.0 - 1.0).abs() < 0.02, "freq {}", band_plain(&host, 1, FREQ.id));
    assert!((band_plain(&host, 1, GAIN.id) + 3.0).abs() < 0.01);
    assert!((band_plain(&host, 1, Q.id) - 3.0).abs() < 0.01);
}

#[test]
fn above_the_spectrum_the_pointer_ignores_its_peaks() {
    let (mut view, host) = with_a_peak();
    // About 12 px above the tip of the tone's peak: within reach of it, but over the line, not
    // under it. A double-click there adds an ordinary bell at the pointer (Q 1), not one on the peak.
    let above = Point::new(PEAK.x + 6.0, PEAK.y - 15.0);
    view.event(&UiEvent::PointerMove { pos: above, mods: mods() }, &host);
    view.event(&UiEvent::DoubleClick { pos: above, mods: mods() }, &host);
    assert_eq!(band_plain(&host, 1, ON.id), 1.0);
    assert!((band_plain(&host, 1, Q.id) - 1.0).abs() < 0.01, "q {}", band_plain(&host, 1, Q.id));
}

/// The header's snapshot letters A–D.
fn slot(i: usize) -> Point {
    Point::new(604.0 + 38.0 * i as f64, 28.0)
}

#[test]
fn snapshot_letters_switch_what_is_edited_and_bring_each_one_back() {
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, GAIN.id), 6.0);
    // B was never stored: it starts as a copy of A.
    click(&mut view, &host, slot(1));
    assert_eq!(plain(&host, GAIN.id), 6.0);
    host.set_plain(&band_id(0, GAIN.id), -4.0);
    draw(&mut view, &host);
    // Back to A: its +6 dB returns; to B again: −4 dB.
    click(&mut view, &host, slot(0));
    assert!((plain(&host, GAIN.id) - 6.0).abs() < 0.01, "A {}", plain(&host, GAIN.id));
    draw(&mut view, &host);
    click(&mut view, &host, slot(1));
    assert!((plain(&host, GAIN.id) + 4.0).abs() < 0.01, "B {}", plain(&host, GAIN.id));
}

#[test]
fn dragging_a_letter_onto_another_copies_that_snapshot() {
    let (mut view, host) = setup();
    host.set_plain(&band_id(0, GAIN.id), 9.0);
    draw(&mut view, &host);
    drag(&mut view, &host, slot(0), slot(2));
    // A is still the one edited; C now holds A.
    host.set_plain(&band_id(0, GAIN.id), 0.0);
    click(&mut view, &host, slot(2));
    assert!((plain(&host, GAIN.id) - 9.0).abs() < 0.01, "C {}", plain(&host, GAIN.id));
}


const UNDO: Point = Point::new(508.0, 28.0);
const REDO: Point = Point::new(544.0, 28.0);
const PRESET_NAME: Point = Point::new(311.0, 28.0);
const PRESET_NEXT: Point = Point::new(456.0, 28.0);

/// Row `k` of the open preset list (0: "Save as…", 1: the factory heading, 2: Init, 3: …).
fn preset_row(k: usize) -> Point {
    Point::new(300.0, 64.0 + 28.0 * k as f64)
}

fn key(view: &mut EqView, host: &MemoryParamHost, key: Key, ctrl: bool, shift: bool) -> bool {
    let mods = Modifiers { ctrl, shift, ..Modifiers::default() };
    view.event(&UiEvent::KeyDown { key, mods }, host)
}

#[test]
fn undo_and_redo_step_through_the_editors_own_edits() {
    let (mut view, host) = setup();
    // One drag is one step.
    drag(&mut view, &host, NODE, Point::new(NODE.x, NODE.y - 88.0));
    let raised = plain(&host, GAIN.id);
    assert!(raised > 5.0, "{raised}");
    click(&mut view, &host, UNDO);
    assert!(plain(&host, GAIN.id).abs() < 0.01, "undone {}", plain(&host, GAIN.id));
    draw(&mut view, &host);
    click(&mut view, &host, REDO);
    assert!((plain(&host, GAIN.id) - raised).abs() < 0.01);
    // The keys do the same.
    assert!(key(&mut view, &host, Key::Character('z'), true, false));
    assert!(plain(&host, GAIN.id).abs() < 0.01);
    assert!(key(&mut view, &host, Key::Character('Z'), true, true));
    assert!((plain(&host, GAIN.id) - raised).abs() < 0.01);
}

#[test]
fn automation_is_not_an_undo_step() {
    let (mut view, host) = setup();
    view.tick(&host);
    // The host moves the gain (automation): nothing to undo.
    host.set_plain(&band_id(0, GAIN.id), 7.0);
    view.tick(&host);
    click(&mut view, &host, UNDO);
    assert_eq!(plain(&host, GAIN.id), 7.0);
}

#[test]
fn presets_load_from_the_list_and_step_with_the_arrows() {
    let (mut view, host) = setup();
    click(&mut view, &host, PRESET_NAME);
    draw(&mut view, &host);
    // Row 3: the first factory preset after Init (vocal: a low cut at 90 Hz in band 1).
    click(&mut view, &host, preset_row(3));
    assert_eq!(band_plain(&host, 0, TYPE.id), 3.0, "a low cut");
    assert!((band_plain(&host, 0, FREQ.id) - 90.0).abs() < 0.5);
    assert_eq!(band_plain(&host, 3, ON.id), 1.0);
    draw(&mut view, &host);
    // The next one replaces it completely (what it leaves out goes back to its default).
    click(&mut view, &host, PRESET_NEXT);
    assert!((band_plain(&host, 0, FREQ.id) - 30.0).abs() < 0.5, "kick preset {}", band_plain(&host, 0, FREQ.id));
    // Loading a preset is an undo step.
    draw(&mut view, &host);
    click(&mut view, &host, UNDO);
    assert!((band_plain(&host, 0, FREQ.id) - 90.0).abs() < 0.5);
}

#[test]
fn save_as_types_a_name_and_writes_a_user_preset() {
    let dir = std::env::temp_dir().join(format!("mitefx-editor-presets-{}", std::process::id()));
    let (view, host) = setup();
    let mut view = view.with_preset_dir(dir.clone());
    host.set_plain(&band_id(0, GAIN.id), 5.0);
    click(&mut view, &host, PRESET_NAME);
    draw(&mut view, &host);
    click(&mut view, &host, preset_row(0));
    // The suggested name cleared, then "Bright".
    for _ in 0..12 {
        key(&mut view, &host, Key::Backspace, false, false);
    }
    for c in "Bright".chars() {
        key(&mut view, &host, Key::Character(c), false, false);
    }
    assert!(key(&mut view, &host, Key::Enter, false, false));
    let saved = mite_eq::presets::user_in(&dir);
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert_eq!(saved[0].name, "Bright");
    assert_eq!(saved[0].values.get("b01_gain").copied(), Some(5.0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The bell from `setup`, made a compressor band through its gain ring.
fn dynamic() -> (EqView, MemoryParamHost) {
    let (mut view, host) = setup();
    let ring = Point::new(762.0 - 35.0, 606.0);
    drag(&mut view, &host, ring, Point::new(ring.x - 40.0, ring.y));
    draw(&mut view, &host);
    assert_eq!(plain(&host, DYN.id), 1.0);
    (view, host)
}


#[test]
fn the_detection_dropdown_offers_the_sidechain_and_the_bands_that_are_on() {
    let (mut view, host) = dynamic();
    host.set_plain(&band_id(4, ON.id), 1.0);
    draw(&mut view, &host);
    // Left of the remove button: own, sidechain, then band 5 (the only other band on).
    let button = Point::new(849.0, 500.0);
    click(&mut view, &host, button);
    draw(&mut view, &host);
    let first = Point::new(860.0, 534.0);
    let row = 30.0;
    click(&mut view, &host, Point::new(first.x, first.y + row));
    assert_eq!(plain(&host, "dsrc"), 1.0, "sidechain");
    draw(&mut view, &host);
    click(&mut view, &host, button);
    draw(&mut view, &host);
    click(&mut view, &host, Point::new(first.x, first.y + 2.0 * row));
    assert_eq!(plain(&host, "dsrc"), 6.0, "band 5 (choice 2 + 4)");
}

/// Noise from a xorshift generator, `n` samples, roughly ±0.3.
fn noise(seed: &mut u32, n: usize) -> Vec<f32> {
    (0..n)
        .map(|_| {
            *seed ^= *seed << 13;
            *seed ^= *seed >> 17;
            *seed ^= *seed << 5;
            (*seed as f32 / u32::MAX as f32 - 0.5) * 0.6
        })
        .collect()
}

/// Feeds `pre`/`post` and the sidechain for about `seconds` of ticks (fresh blocks every tick).
fn play(view: &mut EqView, host: &MemoryParamHost, shared: &EqShared, seconds: f64, track: &mut impl FnMut() -> Vec<f32>, sidechain: &mut impl FnMut() -> Vec<f32>) {
    let start = std::time::Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let t = track();
        shared.pre.push(t.iter().copied());
        shared.post.push(t.iter().copied());
        shared.sidechain.push(sidechain().into_iter());
        std::thread::sleep(std::time::Duration::from_millis(16));
        view.tick(host);
    }
    draw(view, host);
}

const MATCH: Point = Point::new(806.0, 28.0);
/// The Match panel's tick (it sits in the band panel's place, bottom centre).
const MATCH_APPLY: Point = Point::new(921.0, 635.0);

fn enabled(host: &MemoryParamHost) -> Vec<usize> {
    (0..mite_eq::params::BANDS).filter(|&b| band_plain(host, b, ON.id) >= 0.5).collect()
}

#[test]
fn eq_match_learns_a_darker_reference_and_adds_its_bands_as_one_step() {
    let shared = Arc::new(EqShared::new());
    let (mut view, host) = setup_with(shared.clone());
    click(&mut view, &host, MATCH);
    // The reference is the same kind of noise through a one-pole low-pass (about 1.2 kHz).
    let (mut a, mut b) = (1u32, 7u32);
    let mut lp = 0.0f32;
    let mut track = || noise(&mut a, 800);
    let mut reference = || {
        noise(&mut b, 800)
            .into_iter()
            .map(|x| {
                lp += 0.15 * (x - lp);
                lp
            })
            .collect()
    };
    play(&mut view, &host, &shared, 2.0, &mut track, &mut reference);
    click(&mut view, &host, MATCH_APPLY);
    let added: Vec<usize> = enabled(&host).into_iter().filter(|&b| b != 0).collect();
    assert!(!added.is_empty() && added.len() <= 8, "added {added:?}");
    // Together the new bands darken the track: well down at 8 kHz against 200 Hz.
    let at = |f: f64| -> f64 {
        added
            .iter()
            .map(|&b| {
                let s = mite_dsp::eq::BandSettings {
                    enabled: true,
                    kind: BandType::from_index(band_plain(&host, b, TYPE.id) as usize),
                    freq: f64::from(band_plain(&host, b, FREQ.id)),
                    gain_db: f64::from(band_plain(&host, b, GAIN.id)),
                    q: f64::from(band_plain(&host, b, Q.id)),
                    ..Default::default()
                };
                mite_dsp::eq::analog_response_db(&s, f)
            })
            .sum()
    };
    assert!(at(8000.0) < at(200.0) - 10.0, "200 Hz {:.1} dB, 8 kHz {:.1} dB", at(200.0), at(8000.0));
    // One step undoes them all.
    draw(&mut view, &host);
    click(&mut view, &host, UNDO);
    assert_eq!(enabled(&host), vec![0]);
}

#[test]
fn masking_hatches_where_both_peak_and_its_ring_carves_there() {
    let shared = Arc::new(EqShared::new());
    let (mut view, host) = setup_with(shared.clone());
    // Close the band panel; both play a 100 Hz tone over noise.
    key(&mut view, &host, Key::Escape, false, false);
    click(&mut view, &host, Point::new(600.0, 150.0));
    let tone = |phase: &mut f32| -> Vec<f32> {
        (0..800)
            .map(|_| {
                *phase += std::f32::consts::TAU * 100.0 / 48_000.0;
                0.3 * phase.sin()
            })
            .collect()
    };
    let (mut a, mut b, mut pa, mut pb) = (3u32, 9u32, 0.0f32, 0.0f32);
    let mut track = || noise(&mut a, 800).iter().zip(tone(&mut pa)).map(|(n, t)| 0.1 * n + t).collect();
    let mut sidechain = || noise(&mut b, 800).iter().zip(tone(&mut pb)).map(|(n, t)| 0.1 * n + t).collect();
    play(&mut view, &host, &shared, 1.6, &mut track, &mut sidechain);
    let x = 1200.0 * (100.0f64 / 10.0).ln() / 3000.0f64.ln();
    view.event(&UiEvent::PointerMove { pos: Point::new(x, 150.0), mods: mods() }, &host);
    let ring = view.carve_ring().expect("a collision at 100 Hz");
    // Elsewhere there is none.
    view.event(&UiEvent::PointerMove { pos: Point::new(900.0, 150.0), mods: mods() }, &host);
    assert!(view.carve_ring().is_none());
    // Pressing the ring adds the carve: a cut near 100 Hz.
    click(&mut view, &host, ring);
    let added: Vec<usize> = enabled(&host).into_iter().filter(|&b| b != 0).collect();
    assert_eq!(added.len(), 1);
    let freq = band_plain(&host, added[0], FREQ.id);
    assert!((f64::from(freq) / 100.0).log2().abs() < 0.25, "{freq} Hz");
    assert!(band_plain(&host, added[0], GAIN.id) <= -1.5);
    // Right-click on the collision ignores it.
    view.event(&UiEvent::PointerMove { pos: Point::new(x, 150.0), mods: mods() }, &host);
    view.event(&UiEvent::PointerDown { pos: Point::new(x, 150.0), button: MouseButton::Right, mods: mods() }, &host);
    play(&mut view, &host, &shared, 0.3, &mut track, &mut sidechain);
    view.event(&UiEvent::PointerMove { pos: Point::new(x, 150.0), mods: mods() }, &host);
    assert!(view.carve_ring().is_none(), "ignored");
}
