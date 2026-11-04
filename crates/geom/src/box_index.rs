//! A broad phase for boxes in the plane: a uniform grid over them, so asking which boxes hold a
//! point, or meet a box, looks only at the boxes near it. Large layouts (thousands of
//! drilled holes, a sketch of many loops) are then compared in about linear time, where
//! comparing every pair grows with the square of their number.

use std::collections::HashMap;

use crate::Aabb2;
use solidcore_record::Vec2;

/// Most cells one box is filed under; a box over more (or with a side that is not finite) is
/// kept on a list every query looks through.
const MAX_CELLS_PER_BOX: u64 = 64;

/// Most grid cells along a side.
const MAX_PER_SIDE: f64 = 4096.0;

/// The boxes of some items, filed in grid cells by where they lie.
#[derive(Clone, Debug, Default)]
pub struct BoxIndex {
    boxes: Vec<Aabb2>,
    lo: Vec2,
    size: f64,
    cells: HashMap<(i64, i64), Vec<u32>>,
    /// Boxes over too many cells, or not finite: candidates for every query.
    wide: Vec<u32>,
}

impl BoxIndex {
    /// The index of `boxes`; item `i` is `boxes[i]`. An empty box is filed nowhere and is never
    /// a candidate. `None` with more than `u32::MAX` boxes.
    pub fn new(boxes: Vec<Aabb2>) -> Option<BoxIndex> {
        u32::try_from(boxes.len()).ok()?;
        let finite = |b: &Aabb2| !b.is_empty() && b.min.is_finite() && b.max.is_finite();
        let all = boxes.iter().filter(|b| finite(b)).fold(Aabb2::EMPTY, |a, b| a.union(b));
        let count = boxes.iter().filter(|b| finite(b)).count();
        let (lo, size) = if all.is_empty() {
            (Vec2::ZERO, 1.0)
        } else {
            let extent = all.size();
            let side = extent.x.max(extent.y).max(1e-9);
            // About one box per cell, but cells no smaller than the typical box, so that small
            // boxes each go in one to four cells.
            let per_side = (count as f64).sqrt().ceil().clamp(1.0, MAX_PER_SIDE);
            let mut sizes: Vec<f64> = boxes.iter().filter(|b| finite(b)).map(|b| b.size().x.max(b.size().y)).collect();
            let typical = if sizes.is_empty() {
                0.0
            } else {
                let mid = sizes.len() / 2;
                *sizes.select_nth_unstable_by(mid, f64::total_cmp).1
            };
            (all.min, (side / per_side).max(typical).max(side / MAX_PER_SIDE))
        };
        let mut index = BoxIndex { boxes, lo, size, cells: HashMap::new(), wide: Vec::new() };
        for (i, b) in index.boxes.iter().enumerate() {
            let Ok(i) = u32::try_from(i) else { return None };
            if b.is_empty() {
                continue;
            }
            match index.span(b) {
                Some((c0, c1)) => {
                    for x in c0.0..=c1.0 {
                        for y in c0.1..=c1.1 {
                            index.cells.entry((x, y)).or_default().push(i);
                        }
                    }
                }
                None => index.wide.push(i),
            }
        }
        Some(index)
    }

    /// How many boxes the index holds.
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Item `i`'s box.
    pub fn get(&self, i: usize) -> Option<&Aabb2> {
        self.boxes.get(i)
    }

    fn cell(&self, p: Vec2) -> Option<(i64, i64)> {
        let x = ((p.x - self.lo.x) / self.size).floor();
        let y = ((p.y - self.lo.y) / self.size).floor();
        let fits = |v: f64| v.is_finite() && v.abs() < 1e15;
        // In range of an i64 (checked just above), so the conversions are exact.
        (fits(x) && fits(y)).then_some((x as i64, y as i64))
    }

    /// The first and last cell `b` covers, when it covers at most [`MAX_CELLS_PER_BOX`].
    fn span(&self, b: &Aabb2) -> Option<((i64, i64), (i64, i64))> {
        let (c0, c1) = (self.cell(b.min)?, self.cell(b.max)?);
        let side = |lo: i64, hi: i64| u64::try_from(hi.checked_sub(lo)?.checked_add(1)?).ok();
        let count = side(c0.0, c1.0)?.checked_mul(side(c0.1, c1.1)?)?;
        (count <= MAX_CELLS_PER_BOX).then_some((c0, c1))
    }

    /// The items whose box holds `p` (sides included), each once, in increasing order.
    pub fn containing(&self, p: Vec2) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let near = self.cell(p).and_then(|c| self.cells.get(&c)).map(Vec::as_slice).unwrap_or_default();
        for &i in near.iter().chain(&self.wide) {
            let i = i as usize;
            if self.boxes.get(i).is_some_and(|b| b.contains(p)) {
                out.push(i);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The items whose box meets `b` (sides included), each once, in increasing order. A box over
    /// too many cells is compared with every item.
    pub fn meeting(&self, b: &Aabb2) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        if b.is_empty() {
            return out;
        }
        let mut take = |i: u32| {
            let i = i as usize;
            if self.boxes.get(i).is_some_and(|o| o.meets(b)) {
                out.push(i);
            }
        };
        match self.span(b) {
            Some((c0, c1)) => {
                for x in c0.0..=c1.0 {
                    for y in c0.1..=c1.1 {
                        for &i in self.cells.get(&(x, y)).map(Vec::as_slice).unwrap_or_default() {
                            take(i);
                        }
                    }
                }
                for &i in &self.wide {
                    take(i);
                }
            }
            None => (0..self.boxes.len()).filter_map(|i| u32::try_from(i).ok()).for_each(take),
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, s: f64) -> Aabb2 {
        Aabb2 { min: Vec2::new(x, y), max: Vec2::new(x + s, y + s) }
    }

    /// Every query answers exactly what comparing every box would, on small, large, empty and
    /// non-finite boxes.
    #[test]
    fn queries_match_comparing_every_box() {
        let mut boxes = Vec::new();
        for k in 0..2000 {
            boxes.push(square(f64::from(k % 50) * 1.27, f64::from(k / 50) * 1.27, 0.6));
        }
        boxes.push(square(-1.0, -1.0, 100.0));
        boxes.push(Aabb2::EMPTY);
        boxes.push(Aabb2 { min: Vec2::new(f64::NEG_INFINITY, 3.0), max: Vec2::new(4.0, 5.0) });
        let index = BoxIndex::new(boxes.clone()).unwrap();
        for k in 0..500 {
            let p = Vec2::new(f64::from(k) * 0.137 - 2.0, f64::from(k % 37) * 1.9 - 3.0);
            let expected: Vec<usize> = (0..boxes.len()).filter(|i| boxes[*i].contains(p)).collect();
            assert_eq!(index.containing(p), expected, "at {p:?}");
            let q = square(p.x, p.y, f64::from(k % 7) * 0.4);
            let expected: Vec<usize> = (0..boxes.len()).filter(|i| boxes[*i].meets(&q)).collect();
            assert_eq!(index.meeting(&q), expected, "box at {p:?}");
        }
        assert!(index.containing(Vec2::new(f64::NAN, 0.0)).is_empty());
        assert!(BoxIndex::new(Vec::new()).unwrap().containing(Vec2::ZERO).is_empty());
    }
}
