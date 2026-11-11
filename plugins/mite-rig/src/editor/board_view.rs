//! The board as drawn: where each tile, each insertion point, each fork and merge sits, and
//! the wires between them. Pure geometry (no drawing state), so the editor and its tests agree.

use mite_ui::kurbo::{Point, Rect};

use crate::board::{Item, Place};

/// The board's area.
pub const AREA: Rect = Rect::new(0.0, 64.0, 1280.0, 352.0);
pub const TILE_W: f64 = 116.0;
pub const TILE_H: f64 = 76.0;
pub const GAP: f64 = 40.0;
/// Between lanes' centres.
pub const PITCH: f64 = 88.0;
pub const CY: f64 = AREA.y0 + 150.0;
/// IN at the left, OUT at the right of the content.
pub const IN_X: f64 = 34.0;
const FORK_LEAD: f64 = 28.0;

/// Where something goes when it is dropped or added at an insertion point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    pub place: Place,
    pub at: Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub id: u64,
    pub rect: Rect,
    pub place: Place,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fork {
    pub id: u64,
    pub index: usize,
    pub fork: Point,
    pub merge: Point,
    pub lanes: usize,
    /// Each lane's row (centre y) and its start and end x.
    pub rows: [(f64, f64, f64); 3],
}

#[derive(Debug, Clone, Default)]
pub struct Layout {
    pub tiles: Vec<Tile>,
    pub gaps: Vec<Gap>,
    pub forks: Vec<Fork>,
    /// Straight wire runs along the main row: (x0, x1).
    pub runs: Vec<(f64, f64)>,
    pub out_x: f64,
    /// The content's width (for scrolling).
    pub width: f64,
}

impl Layout {
    /// Lays out `items`, scrolled left by `scroll`.
    pub fn new(items: &[Item], scroll: f64) -> Self {
        let mut l = Layout::default();
        let mut x = IN_X + 34.0 - scroll;
        let mut run_from = x - 20.0;
        for (i, item) in items.iter().enumerate() {
            l.gaps.push(Gap { place: Place::Main(i), at: Point::new(x + GAP / 2.0, CY) });
            x += GAP;
            match item {
                Item::Block(b) => {
                    l.tiles.push(Tile { id: b.id(), rect: Rect::new(x, CY - TILE_H / 2.0, x + TILE_W, CY + TILE_H / 2.0), place: Place::Main(i) });
                    x += TILE_W;
                }
                Item::Split(s) => {
                    let n = s.lanes.len();
                    let fork = Point::new(x + 8.0, CY);
                    l.runs.push((run_from, fork.x));
                    let x0 = x + 8.0 + FORK_LEAD;
                    let lane_w = s.lanes.iter().map(|lane| lane.blocks.len() as f64 * (TILE_W + GAP) + GAP).fold(GAP, f64::max);
                    let mut rows = [(0.0, 0.0, 0.0); 3];
                    for (j, lane) in s.lanes.iter().enumerate() {
                        let y = CY + (j as f64 - (n as f64 - 1.0) / 2.0) * PITCH;
                        let mut xl = x0;
                        for (k, b) in lane.blocks.iter().enumerate() {
                            l.gaps.push(Gap { place: Place::Lane { split: i, lane: j, index: k }, at: Point::new(xl + GAP / 2.0, y) });
                            xl += GAP;
                            l.tiles.push(Tile { id: b.id(), rect: Rect::new(xl, y - TILE_H / 2.0, xl + TILE_W, y + TILE_H / 2.0), place: Place::Lane { split: i, lane: j, index: k } });
                            xl += TILE_W;
                        }
                        l.gaps.push(Gap { place: Place::Lane { split: i, lane: j, index: lane.blocks.len() }, at: Point::new(xl + GAP / 2.0, y) });
                        rows[j] = (y, x0, x0 + lane_w);
                    }
                    let merge = Point::new(x0 + lane_w + FORK_LEAD, CY);
                    l.forks.push(Fork { id: s.id, index: i, fork, merge, lanes: n, rows });
                    x = merge.x + 8.0;
                    run_from = merge.x;
                }
            }
        }
        l.gaps.push(Gap { place: Place::Main(items.len()), at: Point::new(x + GAP / 2.0, CY) });
        x += GAP;
        l.runs.push((run_from, x + 10.0));
        l.out_x = x + 34.0;
        l.width = l.out_x + 40.0 + scroll;
        l
    }

    pub fn tile_at(&self, p: Point) -> Option<&Tile> {
        self.tiles.iter().find(|t| t.rect.contains(p))
    }

    pub fn gap_at(&self, p: Point, reach: f64) -> Option<&Gap> {
        self.gaps.iter().filter(|g| g.at.distance(p) <= reach).min_by(|a, b| a.at.distance(p).total_cmp(&b.at.distance(p)))
    }

    pub fn fork_at(&self, p: Point) -> Option<&Fork> {
        self.forks.iter().find(|f| f.fork.distance(p) <= 12.0 || f.merge.distance(p) <= 12.0)
    }

    /// A tile's bypass light.
    pub fn led(tile: &Tile) -> Point {
        Point::new(tile.rect.x1 - 14.0, tile.rect.y0 + 14.0)
    }
}
