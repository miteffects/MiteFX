//! Mite Cut bound to an ARA document holding a whole kit (a host in Rust plays REAPER's part):
//! each track's role comes from its name, one detection serves every instance, a tom track
//! plays its hits and is silent through the snare's bleed, Off passes the clip as it is, a role
//! chosen by hand reaches the host's archive and comes back with it, and the audio thread never
//! allocates.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use mite_ara::testing::{Audio, TestHost};
use mite_cut::dsp::{CutEngine, Transport};
use mite_cut::session::{Session, Status, guess_role};
use mite_dsp::hits::Role;

const NAMES: [&str; 5] = ["Kick In", "Snare Top", "Tom 1", "Floor Tom", "HH"];

/// Plays song time `from`..`from + n` (host playing), input full of 0.5 (which ARA ignores).
fn song(e: &mut CutEngine, s: &Session, from: i64, n: usize, removed: bool) -> Vec<f32> {
    let mut out = Vec::with_capacity(n);
    let mut pos = from;
    while out.len() < n {
        let k = 480.min(n - out.len());
        let (mut l, mut r) = (vec![0.5f32; k], vec![0.5f32; k]);
        e.process(&mut [&mut l, &mut r], Transport { playing: true, pos: Some(pos) }, false, removed, s);
        out.extend_from_slice(&l);
        pos += k as i64;
    }
    out
}

struct Kit {
    host: TestHost,
    mics: Vec<Vec<f32>>,
    truth: Truth,
    regions: Vec<mite_ara::sys::ARAPlaybackRegionRef>,
    mods: Vec<mite_ara::sys::ARAAudioModificationRef>,
}

/// The kit's five mics, each a clip on its own named track from song time 0.
fn kit_in_host(seed: u32) -> Kit {
    let (mics, truth) = kit(seed);
    let mut host = TestHost::new(mite_ara::factory(&mite_cut::ara::INFO));
    let mut regions = Vec::new();
    let mut mods = Vec::new();
    for (i, (m, name)) in mics.iter().zip(NAMES).enumerate() {
        let seq = host.add_track(name);
        let src = mite_ara::document::leak(&format!("src-{seed}-{i}"));
        let md = mite_ara::document::leak(&format!("mod-{seed}-{i}"));
        let dur = m.len() as f64 / f64::from(FS);
        let (_s, md, rg) = host.add_clip_to(seq, Audio { fs: f64::from(FS), l: m.clone(), r: m.clone() }, src, md, 0.0, 0.0, dur);
        regions.push(rg);
        mods.push(md);
    }
    Kit { host, mics, truth, regions, mods }
}

/// An instance on track `i`, bound with `roles` (7: all; 1: playback only), given its clip.
fn instance(k: &Kit, i: usize, roles: i32) -> (Arc<Session>, CutEngine) {
    let s = Session::new();
    s.set_sample_rate(FS);
    let inst = unsafe { mite_ara::bind(k.host.dc, 7, roles) };
    k.host.give(inst.extension(), k.regions[i]);
    assert!(inst.document.wait_reads(Duration::from_secs(20)));
    s.bind_ara(inst);
    assert!(s.wait_idle(Duration::from_secs(60)));
    let mut e = CutEngine::new(FS);
    song(&mut e, &s, 0, 480, false);
    (s, e)
}

/// The peak of `x` over 20 ms from `at`.
fn peak(x: &[f32], at: usize) -> f32 {
    x[at..(at + 960).min(x.len())].iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn names_say_the_role() {
    for (name, want) in [("Kick In", Role::Kick), ("KICK OUT", Role::Kick), ("Snare Top", Role::Snare), ("snare btm", Role::Snare), ("Tom 1", Role::Tom), ("tom2", Role::Tom), ("Floor Tom", Role::Tom), ("HH", Role::Hat), ("Hi-Hat", Role::Hat), ("OH L", Role::Overhead), ("Overheads", Role::Overhead), ("Room Mono", Role::Room), ("Ride", Role::Cymbal), ("Bass DI", Role::Off), ("Vox", Role::Off), ("Audio 7", Role::Off)] {
        assert_eq!(guess_role(name), want, "{name}");
    }
}

#[test]
fn a_kit_in_one_document_cuts_each_track_against_the_others() {
    let k = kit_in_host(91);
    let (tom, mut e_tom) = instance(&k, 2, 7);
    let (snare, mut e_snare) = instance(&k, 1, 7);
    assert_eq!(tom.status(), Status::Ready);
    // One detection for the document, whichever instance asked first.
    let made = tom.detections_made.load(std::sync::atomic::Ordering::Acquire) + snare.detections_made.load(std::sync::atomic::Ordering::Acquire);
    assert_eq!(made, 1, "the instances share one detection");
    let tracks = tom.tracks();
    let roles: Vec<(String, Role, bool)> = tracks.iter().map(|t| (t.name.clone(), t.role, t.guessed)).collect();
    println!("{roles:?}");
    for (name, want) in NAMES.iter().zip([Role::Kick, Role::Snare, Role::Tom, Role::Tom, Role::Hat]) {
        assert!(roles.iter().any(|(n, r, g)| n == name && *r == want && *g), "{name}: {roles:?}");
    }
    let t1 = tracks.iter().find(|t| t.name == "Tom 1").expect("tom 1");
    let (kept, all) = t1.counts();
    println!("tom 1: {kept} of {all} kept");
    // The tom track: its hits sound as they are, the snare's bleed between them is silent.
    let n = k.mics[2].len();
    let out = song(&mut e_tom, &tom, 0, n, false);
    let toms: Vec<usize> = k.truth.iter().filter(|t| t.0 == Drum::Tom1).map(|t| t.1).collect();
    let near_tom = |at: usize| toms.iter().any(|&t| at + 2400 > t && at < t + (0.6 * FS) as usize);
    for &t in &toms {
        let (o, x) = (peak(&out, t), peak(&k.mics[2], t));
        assert!((o - x).abs() < 1e-3 * x.max(1e-3), "tom hit at {t}: {o} against {x}");
    }
    // (Away from every region kept for a real tom hit and its fade: a tom rings for most of a
    // second.)
    let fade = (0.04 * FS) as usize;
    let tom_regions: Vec<(usize, usize)> = t1.clips[0].hits.iter().filter(|h| h.kept && toms.iter().any(|&t| h.onset.abs_diff(t) < 240)).map(|h| (h.start, h.end + fade)).collect();
    let mut silent = 0;
    let mut bleeds = 0;
    for b in k.truth.iter().filter(|t| t.0 == Drum::Snare && !near_tom(t.1)) {
        let at = b.1 + 200;
        if tom_regions.iter().any(|&(a, e)| at + 960 > a && at < e) {
            continue;
        }
        bleeds += 1;
        if peak(&out, at) < 1e-6 {
            silent += 1;
        }
    }
    println!("snare bleed on the tom track, away from its toms: {silent} of {bleeds} silent");
    assert!(bleeds > 10 && silent == bleeds);
    // The snare's own track keeps its hits.
    let out = song(&mut e_snare, &snare, 0, n, false);
    for s in k.truth.iter().filter(|t| t.0 == Drum::Snare) {
        assert!(peak(&out, s.1) > 0.5 * peak(&k.mics[1], s.1), "snare hit at {}", s.1);
    }
    // Listen Removed: the tom track's cut, and nothing of the toms' attacks.
    let removed = song(&mut e_tom, &tom, 0, n, true);
    let worst = toms.iter().map(|&t| peak(&removed, t + 100) / peak(&k.mics[2], t + 100)).fold(0.0, f32::max);
    assert!(worst < 0.01, "a tom's attack in what is removed: {worst}");
    let away = |at: usize| !near_tom(at) && !tom_regions.iter().any(|&(a, e)| at + 960 > a && at < e);
    assert!(k.truth.iter().filter(|t| t.0 == Drum::Snare && away(t.1 + 200)).all(|b| peak(&removed, b.1 + 200) > 0.0), "the bleed is what is removed");
    assert_eq!(mite_ara::testing::violations(), Vec::<String>::new(), "ARA's threading rules");
}

#[test]
fn a_role_set_by_hand_plays_and_comes_back_with_the_project() {
    let k = kit_in_host(7);
    let (s, mut e) = instance(&k, 2, 7);
    let seq = s.tracks().iter().find(|t| t.name == "Tom 1").expect("tom 1").sequence;
    s.set_role(seq, Role::Off);
    assert!(s.wait_idle(Duration::from_secs(60)));
    s.sync_ara();
    assert!(s.wait_idle(Duration::from_secs(60)));
    let t = s.tracks().iter().find(|t| t.sequence == seq).cloned().expect("tom 1");
    assert_eq!((t.role, t.guessed), (Role::Off, false));
    // Off: the clip as it is (after the crossfade in).
    let n = k.mics[2].len();
    let out = song(&mut e, &s, 0, n, false);
    let worst = out[4800..].iter().zip(&k.mics[2][4800..]).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-6, "{worst}");
    // The host's archive holds it; a fresh document restored from it has it.
    k.host.notify_model_updates();
    let archive = k.host.store(&k.mods);
    let again = kit_in_host(7);
    assert!(again.host.restore(&archive));
    again.host.notify_model_updates();
    let (s2, _e2) = instance(&again, 2, 7);
    let t2 = s2.tracks().iter().find(|t| t.name == "Tom 1").cloned().expect("tom 1");
    assert_eq!((t2.role, t2.guessed), (Role::Off, false));
}

#[test]
fn an_editor_instance_passes_and_the_player_never_allocates() {
    let k = kit_in_host(3);
    let (ed, mut e_ed) = instance(&k, 3, 6);
    let (pl, mut e_pl) = instance(&k, 3, 1);
    let through = song(&mut e_ed, &ed, 0, 48_000, false);
    assert!(through.iter().all(|v| *v == 0.5), "the editor instance passes its input");
    let (mut l, mut r) = (vec![0.0f32; 512], vec![0.0f32; 512]);
    for k in 0..400 {
        let n = if k % 3 == 0 { 512 } else { 77 };
        let t = Transport { playing: true, pos: Some(k as i64 * 512) };
        assert_no_alloc::assert_no_alloc(|| e_pl.process(&mut [&mut l[..n], &mut r[..n]], t, k % 50 < 10, k % 70 < 20, &pl));
        if k == 200 {
            // A new cut arriving mid-play (another sensitivity), and leaving.
            let seq = pl.tracks().iter().find(|t| t.name == "Floor Tom").expect("floor tom").sequence;
            pl.set_settings(seq, mite_dsp::hits::Settings { sensitivity: 0.8, ..Default::default() });
            assert!(pl.wait_idle(Duration::from_secs(60)));
        }
    }
    let stopped = Transport { playing: false, pos: Some(0) };
    assert_no_alloc::assert_no_alloc(|| e_pl.process(&mut [&mut l[..64], &mut r[..64]], stopped, false, false, &pl));
}
