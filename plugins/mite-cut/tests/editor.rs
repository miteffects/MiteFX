//! The editor driven as a user would: a role chip clicked moves the track's role on, a hit's
//! mark clicked cuts it (and the cut that plays follows), a double-click puts it back.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use mite_ara::testing::{Audio, TestHost};
use mite_cut::CutShared;
use mite_cut::editor::CutView;
use mite_cut::session::Session;
use mite_dsp::hits::Role;
use mite_ui::{Modifiers, MouseButton, UiEvent, View};

fn settle(s: &Session) {
    assert!(s.wait_idle(Duration::from_secs(60)));
    s.sync_ara();
    assert!(s.wait_idle(Duration::from_secs(60)));
}

#[test]
fn chips_and_marks_edit_the_cut() {
    let (mics, _truth) = kit(91);
    let mut host = TestHost::new(mite_ara::factory(&mite_cut::ara::INFO));
    let mut first = None;
    for (i, (m, name)) in mics.iter().zip(["Kick In", "Snare Top", "Tom 1", "Floor Tom", "HH"]).enumerate() {
        let seq = host.add_track(name);
        let (_s, _m, rg) = host.add_clip_to(seq, Audio { fs: f64::from(FS), l: m.clone(), r: m.clone() }, mite_ara::document::leak(&format!("e-src-{i}")), mite_ara::document::leak(&format!("e-mod-{i}")), 0.0, 0.0, m.len() as f64 / f64::from(FS));
        if i == 2 {
            first = Some(rg);
        }
    }
    let s = Session::new();
    s.set_sample_rate(FS);
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    host.give(inst.extension(), first.expect("tom 1"));
    assert!(inst.document.wait_reads(Duration::from_secs(20)));
    s.bind_ara(inst);
    settle(&s);
    let params = mite_cut::editor::memory_host();
    let mut view = CutView::new(Arc::new(CutShared { session: s.clone() }));
    view.tick(&params);
    let tom = s.tracks().iter().find(|t| t.name == "Tom 1").cloned().expect("tom 1");
    let click = |view: &mut CutView, p: mite_ui::kurbo::Point, shift: bool| {
        let mods = Modifiers { shift, ..Modifiers::default() };
        view.event(&UiEvent::PointerMove { pos: p, mods }, &params);
        view.event(&UiEvent::PointerDown { pos: p, button: MouseButton::Left, mods }, &params);
        view.event(&UiEvent::PointerUp { pos: p, button: MouseButton::Left, mods }, &params);
    };
    // A kept hit: clicked, it is cut (by hand), and the cut that plays is closed there.
    let i = tom.clips[0].hits.iter().position(|h| h.kept && !h.inside).expect("a kept hit");
    let h = tom.clips[0].hits[i];
    let p = view.mark_point(tom.sequence, 0, i).expect("its mark");
    click(&mut view, p, false);
    settle(&s);
    let now = s.tracks().iter().find(|t| t.sequence == tom.sequence).cloned().expect("tom 1");
    let after = now.clips[0].hits.iter().find(|x| x.onset == h.onset).copied().expect("the hit");
    assert!(!after.kept && after.forced, "{after:?}");
    assert!(now.clips[0].curve.at(h.onset + 200) < 1e-3, "the cut closed over it");
    // A double-click: as detected again.
    view.tick(&params);
    let p = view.mark_point(tom.sequence, 0, now.clips[0].hits.iter().position(|x| x.onset == h.onset).expect("hit")).expect("mark");
    view.event(&UiEvent::DoubleClick { pos: p, mods: Modifiers::default() }, &params);
    settle(&s);
    let back = s.tracks().iter().find(|t| t.sequence == tom.sequence).cloned().expect("tom 1");
    let again = back.clips[0].hits.iter().find(|x| x.onset == h.onset).copied().expect("the hit");
    assert!(again.kept && !again.forced, "{again:?}");
    // The role chip: Tom → Hat; Shift-click → Tom again.
    view.tick(&params);
    let chip = view.chip_rect(tom.sequence).expect("chip").center();
    click(&mut view, chip, false);
    settle(&s);
    assert_eq!(s.tracks().iter().find(|t| t.sequence == tom.sequence).expect("tom 1").role, Role::Hat);
    click(&mut view, chip, true);
    settle(&s);
    let t = s.tracks().iter().find(|t| t.sequence == tom.sequence).cloned().expect("tom 1");
    assert_eq!((t.role, t.guessed), (Role::Tom, false));
}
