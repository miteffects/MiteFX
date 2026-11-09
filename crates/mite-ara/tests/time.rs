//! The song's time from the host: the musical context's tempo map and bar signatures are read
//! (on the model thread) when the context is made and whenever the host says they changed, and a
//! region finds them through its track.

use mite_ara::testing::{violations, Audio, TestHost};
use mite_ara::FactoryInfo;

static INFO: FactoryInfo = FactoryInfo {
    plugin_name: c"Mite Test",
    manufacturer: c"MiteFX",
    url: c"https://example.com",
    version: c"0.1.0",
    factory_id: c"io.github.ansabic.mitefx.time.ara",
    archive_id: c"io.github.ansabic.mitefx.time.archive.1",
};

#[test]
fn the_tempo_map_and_bar_signatures_reach_the_document() {
    let mut host = TestHost::new(mite_ara::factory(&INFO));
    let x = vec![0.0f32; 4800];
    let (_s, _m, rg) = host.add_clip(Audio { fs: 48_000.0, l: x.clone(), r: x }, c"src-t", c"mod-t", 0.0, 0.0, 0.1);
    let inst = unsafe { mite_ara::bind(host.dc, 7, 7) };
    let doc = inst.document.clone();
    // Nothing given yet: no tempo.
    let m = doc.snapshot();
    let region = &m.regions[&mite_ara::document::region_id(rg)];
    assert!(m.context_of(region).expect("its context").tempo.is_empty());
    // 120 bpm, then 90 from quarter 8; 4/4, then 3/4 from quarter 16.
    host.set_time(&[(0.0, 0.0), (4.0, 8.0), (4.0 + 2.0 / 1.5, 10.0)], &[(4, 4, 0.0), (3, 4, 16.0)]);
    let m = doc.snapshot();
    let c = m.context_of(&m.regions[&mite_ara::document::region_id(rg)]).expect("its context");
    assert_eq!(c.tempo, vec![(0.0, 0.0), (4.0, 8.0), (4.0 + 2.0 / 1.5, 10.0)]);
    assert_eq!(c.bars, vec![(4, 4, 0.0), (3, 4, 16.0)]);
    assert_eq!(violations(), Vec::<String>::new());
}
