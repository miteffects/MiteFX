//! The board as the audio thread runs it: stages in a row, each a block or a split whose lanes
//! run side by side and merge (level and pan per lane), the shorter lanes delayed to the longest
//! so they stay in time.
//!
//! A graph is built on the loader's thread for each structural change and swapped in by the
//! engine; [`Graph::adopt`] moves the running units of blocks that are still on the board into
//! the new graph (their models, their delay tails), so an edit elsewhere changes nothing about
//! them. Allocation-free on the audio thread.

use std::sync::Arc;

use mite_dsp::LinearSmoother;

use crate::blocks::{self, Unit};
use crate::board::{Item, LaneLive, Live, SplitLive};
use crate::parts::{Delay, MAX_LATENCY, SUB};

pub struct Node {
    pub id: u64,
    pub live: Arc<Live>,
    pub unit: Box<dyn Unit>,
    bypass: LinearSmoother,
    /// The input, as late as the unit makes its output (a bypass stays in time).
    dry: [Delay; 2],
    tmp: [Vec<f32>; 2],
}

impl Node {
    fn new(live: Arc<Live>, fs: f32) -> Self {
        let mut bypass = LinearSmoother::new(if live.bypassed() { 1.0 } else { 0.0 });
        bypass.set_ramp(fs, 15.0);
        let mut unit = blocks::build(&live, fs);
        unit.reset(&live);
        Self { id: live.id, unit, bypass, dry: [Delay::new(MAX_LATENCY), Delay::new(MAX_LATENCY)], tmp: [vec![0.0; SUB], vec![0.0; SUB]], live }
    }

    fn latency(&self) -> usize {
        self.unit.latency().min(MAX_LATENCY)
    }

    fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        self.tmp[0][..n].copy_from_slice(l);
        self.tmp[1][..n].copy_from_slice(r);
        self.bypass.set_target(if self.live.bypassed() { 1.0 } else { 0.0 });
        // Fully bypassed and settled: the unit still runs (its state stays warm, its file
        // swaps still land), and its output is not used.
        self.unit.process(l, r, &self.live);
        let lat = self.latency();
        let [dl, dr] = &mut self.dry;
        dl.delay = lat;
        dr.delay = lat;
        for i in 0..n {
            let b = self.bypass.next_value();
            let (xl, xr) = (dl.tick(self.tmp[0][i]), dr.tick(self.tmp[1][i]));
            l[i] += b * (xl - l[i]);
            r[i] += b * (xr - r[i]);
        }
    }

    fn reset(&mut self) {
        self.unit.reset(&self.live);
        self.dry.iter_mut().for_each(Delay::reset);
    }
}

pub struct Lane {
    pub nodes: Vec<Node>,
    live: Arc<LaneLive>,
    comp: [Delay; 2],
    l: Vec<f32>,
    r: Vec<f32>,
    level: LinearSmoother,
    pan: LinearSmoother,
}

impl Lane {
    fn latency(&self) -> usize {
        self.nodes.iter().map(Node::latency).sum::<usize>().min(MAX_LATENCY)
    }
}

pub enum Stage {
    Block(Node),
    Split {
        lanes: Vec<Lane>,
        sum: [Vec<f32>; 2],
        live: Arc<SplitLive>,
        /// The input, delayed as the longest lane, for the mix.
        dry: [Delay; 2],
        mix: LinearSmoother,
        /// A short dip while the routing changes (no click), and the routing in force.
        dip: LinearSmoother,
        routing: usize,
    },
}

pub struct Graph {
    pub stages: Vec<Stage>,
}

/// A lane's balance gains (pan −1…1: the far side falls, the near one stays at unity).
fn balance(p: f32) -> (f32, f32) {
    let p = p.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

impl Graph {
    /// The board's items as a graph at `fs`. Allocates; worker thread.
    pub fn build(items: &[Item], fs: f32) -> Self {
        let stages = items
            .iter()
            .map(|item| match item {
                Item::Block(b) => Stage::Block(Node::new(b.live.clone(), fs)),
                Item::Split(s) => Stage::Split {
                    lanes: s
                        .lanes
                        .iter()
                        .map(|lane| {
                            let smoother = |v: f32| {
                                let mut sm = LinearSmoother::new(v);
                                sm.set_ramp(fs, 20.0);
                                sm
                            };
                            Lane {
                                nodes: lane.blocks.iter().map(|b| Node::new(b.live.clone(), fs)).collect(),
                                level: smoother(mite_dsp::db_to_gain(lane.live.level_db.load())),
                                pan: smoother(lane.live.pan.load()),
                                live: lane.live.clone(),
                                comp: [Delay::new(MAX_LATENCY), Delay::new(MAX_LATENCY)],
                                l: vec![0.0; SUB],
                                r: vec![0.0; SUB],
                            }
                        })
                        .collect(),
                    sum: [vec![0.0; SUB], vec![0.0; SUB]],
                    dry: [Delay::new(MAX_LATENCY), Delay::new(MAX_LATENCY)],
                    mix: {
                        let mut sm = LinearSmoother::new(s.live.mix.load());
                        sm.set_ramp(fs, 20.0);
                        sm
                    },
                    dip: {
                        let mut sm = LinearSmoother::new(1.0);
                        sm.set_ramp(fs, 5.0);
                        sm
                    },
                    routing: s.live.routing(),
                    live: s.live.clone(),
                },
            })
            .collect();
        Self { stages }
    }

    /// The input-to-output delay, samples: the blocks' latencies along the longest path.
    pub fn latency(&self) -> usize {
        self.stages
            .iter()
            .map(|s| match s {
                Stage::Block(n) => n.latency(),
                Stage::Split { lanes, .. } => lanes.iter().map(Lane::latency).max().unwrap_or(0),
            })
            .sum()
    }

    /// The node of block `id` (loops, not boxed iterators: this runs on the audio thread).
    fn find(&mut self, id: u64) -> Option<&mut Node> {
        for stage in self.stages.iter_mut() {
            match stage {
                Stage::Block(n) if n.id == id => return Some(n),
                Stage::Block(_) => {}
                Stage::Split { lanes, .. } => {
                    for lane in lanes.iter_mut() {
                        for n in lane.nodes.iter_mut() {
                            if n.id == id {
                                return Some(n);
                            }
                        }
                    }
                }
            }
        }
        None
    }

    /// The running units of blocks still on the board move from `old` into this graph (the
    /// fresh ones built for them go back in their place, to be freed with `old`). A block that
    /// had a file but whose unit was not found asks the loader to send it again.
    /// Allocation-free: only boxes move.
    pub fn adopt(&mut self, old: &mut Graph) {
        for stage in self.stages.iter_mut() {
            let nodes: &mut [Node] = match stage {
                Stage::Block(n) => std::slice::from_mut(n),
                Stage::Split { lanes, .. } => {
                    for lane in lanes.iter_mut() {
                        for n in lane.nodes.iter_mut() {
                            adopt_one(n, old);
                        }
                    }
                    continue;
                }
            };
            for n in nodes {
                adopt_one(n, old);
            }
        }
    }

    pub fn reset(&mut self) {
        for stage in self.stages.iter_mut() {
            match stage {
                Stage::Block(n) => n.reset(),
                Stage::Split { lanes, dry, .. } => {
                    for lane in lanes.iter_mut() {
                        lane.nodes.iter_mut().for_each(Node::reset);
                        lane.comp.iter_mut().for_each(Delay::reset);
                    }
                    dry.iter_mut().for_each(Delay::reset);
                }
            }
        }
    }

    /// Whether a block plays a full rig.
    pub fn full_rig(&mut self, id: u64) -> bool {
        self.find(id).is_some_and(|n| n.unit.full_rig())
    }

    /// `l`, `r` (≤ [`SUB`]) through the board, in place.
    pub fn process(&mut self, l: &mut [f32], r: &mut [f32]) {
        let n = l.len();
        for stage in self.stages.iter_mut() {
            match stage {
                Stage::Block(node) => node.process(l, r),
                Stage::Split { lanes, sum, live, dry, mix, dip, routing } => {
                    let longest = lanes.iter().map(Lane::latency).max().unwrap_or(0);
                    // A new routing: down, switch, up again.
                    let wanted = live.routing();
                    if wanted != *routing {
                        if dip.target() > 0.0 {
                            dip.set_target(0.0);
                        } else if !dip.is_smoothing() {
                            *routing = wanted;
                            dip.set_target(1.0);
                        }
                    }
                    mix.set_target(live.mix.load().clamp(0.0, 1.0));
                    sum[0][..n].fill(0.0);
                    sum[1][..n].fill(0.0);
                    for (j, lane) in lanes.iter_mut().enumerate() {
                        // L/R and M/S: the first two lanes take one side each (on both channels);
                        // a third lane takes everything.
                        let side = if j < 2 { *routing } else { 0 };
                        for i in 0..n {
                            let (a, b) = match side {
                                1 => {
                                    let x = if j == 0 { l[i] } else { r[i] };
                                    (x, x)
                                }
                                2 => {
                                    let x = if j == 0 { 0.5 * (l[i] + r[i]) } else { 0.5 * (l[i] - r[i]) };
                                    (x, x)
                                }
                                _ => (l[i], r[i]),
                            };
                            lane.l[i] = a;
                            lane.r[i] = b;
                        }
                        let lat = lane.latency();
                        let (ll, lr) = (&mut lane.l[..n], &mut lane.r[..n]);
                        for node in lane.nodes.iter_mut() {
                            node.process(ll, lr);
                        }
                        let [cl, cr] = &mut lane.comp;
                        cl.delay = longest - lat;
                        cr.delay = longest - lat;
                        lane.level.set_target(mite_dsp::db_to_gain(lane.live.level_db.load()));
                        lane.pan.set_target(lane.live.pan.load());
                        for i in 0..n {
                            let (gl, gr) = balance(lane.pan.next_value());
                            let g = lane.level.next_value();
                            let (yl, yr) = (cl.tick(ll[i]) * g, cr.tick(lr[i]) * g);
                            match side {
                                // Each side back where it came from (its pan does not apply).
                                1 => sum[j][i] += 0.5 * (yl + yr),
                                2 => {
                                    let m = 0.5 * (yl + yr);
                                    let sgn = if j == 0 { 1.0 } else { -1.0 };
                                    sum[0][i] += m;
                                    sum[1][i] += sgn * m;
                                }
                                _ => {
                                    sum[0][i] += yl * gl;
                                    sum[1][i] += yr * gr;
                                }
                            }
                        }
                    }
                    let [dl, dr] = dry;
                    dl.delay = longest;
                    dr.delay = longest;
                    for i in 0..n {
                        let (m, d) = (mix.next_value(), dip.next_value());
                        let (xl, xr) = (dl.tick(l[i]), dr.tick(r[i]));
                        l[i] = xl + m * (d * sum[0][i] - xl);
                        r[i] = xr + m * (d * sum[1][i] - xr);
                    }
                }
            }
        }
    }
}

fn adopt_one(n: &mut Node, old: &mut Graph) {
    let found = match old.find(n.id) {
        // The same block (not just its id: a preset or undo may put another there).
        Some(o) if Arc::ptr_eq(&o.live, &n.live) => {
            std::mem::swap(&mut n.unit, &mut o.unit);
            std::mem::swap(&mut n.dry, &mut o.dry);
            std::mem::swap(&mut n.bypass, &mut o.bypass);
            true
        }
        _ => false,
    };
    if !found && n.live.kind.has_file() && n.live.has_path() && !n.unit.holds_file() {
        n.live.reload.store(true, std::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::Kind;
    use crate::board::{Board, Place};

    fn run(g: &mut Graph, x: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut l, mut r) = (x.to_vec(), x.to_vec());
        for (a, b) in l.chunks_mut(SUB).zip(r.chunks_mut(SUB)) {
            g.process(a, b);
        }
        (l, r)
    }

    #[test]
    fn a_split_of_neutral_lanes_sums_them_and_pans_them() {
        let board = Board::new();
        board.insert_split(0, 3);
        let items = board.items();
        if let Item::Split(s) = &items[0] {
            s.lanes[0].live.pan.store(-1.0);
            s.lanes[1].live.pan.store(1.0);
            s.lanes[2].live.level_db.store(-120.0);
        }
        let mut g = Graph::build(&items, 48_000.0);
        let x: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin() * 0.2).collect();
        let (l, r) = run(&mut g, &x);
        // Left: lane 0 (full) and lane 1 (panned away: 0); lane 2 silent. Right likewise.
        let err = |v: &[f32]| v.iter().zip(&x).skip(2000).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(err(&l) < 1e-3 && err(&r) < 1e-3, "{} {}", err(&l), err(&r));
    }

    #[test]
    fn a_rebuilt_graph_keeps_the_running_units_of_blocks_still_there() {
        let board = Board::with_default();
        let keep = board.blocks()[1].id();
        let mut old = Graph::build(&board.items(), 48_000.0);
        // Mark the old unit: its gain smoother state differs after a run with a value change.
        let boost = board.insert(Kind::Boost, Place::Main(0)).unwrap();
        let mut new = Graph::build(&board.items(), 48_000.0);
        let before: *const dyn Unit = old.find(keep).map(|n| &*n.unit as *const dyn Unit).unwrap();
        new.adopt(&mut old);
        let after: *const dyn Unit = new.find(keep).map(|n| &*n.unit as *const dyn Unit).unwrap();
        assert!(std::ptr::addr_eq(before, after), "the same unit moved over");
        assert!(new.find(boost).is_some());
    }

    /// L/R and M/S with empty lanes give the input back exactly; a mix of 0 is the dry input.
    #[test]
    fn a_split_routes_left_right_and_mid_side_and_mixes() {
        for (routing, mix) in [(1u32, 1.0f32), (2, 1.0), (0, 0.0), (2, 0.0)] {
            let board = Board::new();
            board.insert_split(0, 2);
            let items = board.items();
            if let Item::Split(s) = &items[0] {
                s.live.routing.store(routing, std::sync::atomic::Ordering::Relaxed);
                s.live.mix.store(mix);
                // Pans that would spoil a parallel sum (L/R and M/S ignore them).
                s.lanes[0].live.pan.store(0.7);
                s.lanes[1].live.pan.store(-0.3);
            }
            let mut g = Graph::build(&items, 48_000.0);
            let xl: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.01).sin() * 0.2).collect();
            let xr: Vec<f32> = (0..4800).map(|i| (i as f32 * 0.023).cos() * 0.3).collect();
            let (mut l, mut r) = (xl.clone(), xr.clone());
            for (a, b) in l.chunks_mut(SUB).zip(r.chunks_mut(SUB)) {
                g.process(a, b);
            }
            let err = |v: &[f32], x: &[f32]| v.iter().zip(x).skip(2000).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(err(&l, &xl) < 1e-4 && err(&r, &xr) < 1e-4, "routing {routing} mix {mix}: {} {}", err(&l, &xl), err(&r, &xr));
        }
    }

    /// Changing the routing while it plays dips and comes back (no step).
    #[test]
    fn a_routing_change_is_click_free() {
        let board = Board::new();
        board.insert_split(0, 2);
        let items = board.items();
        let Item::Split(s) = &items[0] else { panic!() };
        let live = s.live.clone();
        let mut g = Graph::build(&items, 48_000.0);
        let x: Vec<f32> = (0..9600).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        for (k, (a, b)) in l.chunks_mut(SUB).zip(r.chunks_mut(SUB)).enumerate() {
            if k == 10 {
                live.routing.store(2, std::sync::atomic::Ordering::Relaxed);
            }
            g.process(a, b);
        }
        let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.02, "{jump}");
    }
}
