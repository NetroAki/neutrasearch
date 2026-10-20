//! Top-k selection over the flat columns, in any catalog sort order.
use super::Columns;
use rayon::prelude::*;
use std::collections::BinaryHeap;

const CHUNK: usize = 1 << 20;
/// Rows a sequential walk may examine before the parallel scan takes over.
// Tests shrink it so the parallel fallback and chunk resumption get exercised.
const WALK_BUDGET: usize = if cfg!(test) { 2 } else { 1 << 18 };

/// What the first component of the ordering value is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::browse) enum Order {
    Size,
    Mtime,
    /// Id order, which is path order.
    Path,
    /// Position in the case-insensitive name order.
    Name,
}

impl Order {
    /// Orders whose key is the visiting position, so a walk yields them sorted.
    fn sequential(self) -> bool {
        matches!(self, Self::Path | Self::Name)
    }
}

/// Contiguous ids or a window of the name order.
#[derive(Clone, Copy, Debug)]
pub(in crate::browse) enum Span {
    Ids(u32, u32),
    Perm(usize, usize),
}

impl Span {
    fn bounds(self) -> (usize, usize) {
        match self {
            Self::Ids(low, high) => (low as usize, high as usize),
            Self::Perm(low, high) => (low, high),
        }
    }

    fn with_bounds(self, low: usize, high: usize) -> Self {
        match self {
            Self::Ids(..) => Self::Ids(low as u32, high as u32),
            Self::Perm(..) => Self::Perm(low, high),
        }
    }
}

/// First row kept when resuming after an anchor: ascending scans keep values
/// at or above it, descending scans keep values below it. Sequential orders
/// carry the position in `key` and leave `bound` zero.
#[derive(Clone, Copy, Debug)]
pub(in crate::browse) struct Boundary {
    pub(in crate::browse) key: u64,
    pub(in crate::browse) bound: u32,
}

impl Boundary {
    fn packed(self) -> u128 {
        (u128::from(self.key) << 32) | u128::from(self.bound)
    }

    /// Exclusive limit on ordering values; None excludes nothing.
    fn floor(self, descending: bool) -> Option<u128> {
        if descending {
            Some(!self.packed())
        } else {
            self.packed().checked_sub(1)
        }
    }
}

impl Columns {
    fn key(&self, order: Order, id: u32, position: usize) -> u64 {
        match order {
            Order::Size => self.size(id),
            Order::Mtime => u64::from(self.mtime(id)),
            Order::Path => u64::from(id),
            Order::Name => position as u64,
        }
    }

    fn visit(&self, span: Span, mut each: impl FnMut(usize, u32)) {
        match span {
            Span::Ids(low, high) => (low..high).for_each(|id| each(id as usize, id)),
            Span::Perm(low, high) => (low..high).for_each(|at| each(at, self.perm(at))),
        }
    }

    /// Smallest ordering values first. The value packs key and id so the
    /// path tie-break (id order) is exact; descending scans invert it.
    fn ordered(key: u64, id: u32, descending: bool) -> u128 {
        let packed = (u128::from(key) << 32) | u128::from(id);
        if descending {
            !packed
        } else {
            packed
        }
    }

    /// Up to `limit` accepted rows after the boundary, in scan order, each as
    /// (ordering value, id).
    pub(in crate::browse) fn top(
        &self,
        spans: &[Span],
        order: Order,
        descending: bool,
        limit: usize,
        after: Option<Boundary>,
        accept: &(dyn Fn(u32) -> bool + Sync),
    ) -> Vec<(u128, u32)> {
        if limit == 0 {
            return Vec::new();
        }
        if order.sequential() {
            if let Some(found) = self.walk(spans, order, descending, limit, after, accept) {
                return found;
            }
        }
        let floor = after.and_then(|boundary| boundary.floor(descending));
        if after.is_some() && floor.is_none() {
            return Vec::new();
        }
        let mut tasks = Vec::new();
        for span in spans {
            let (low, high) = span.bounds();
            let mut at = low;
            while at < high {
                let end = (at + CHUNK).min(high);
                tasks.push(span.with_bounds(at, end));
                at = end;
            }
        }
        let best = tasks
            .par_iter()
            .fold(BinaryHeap::<(u128, u32)>::new, |mut heap, span| {
                self.visit(*span, |position, id| {
                    let value = Self::ordered(self.key(order, id, position), id, descending);
                    if floor.is_some_and(|floor| value <= floor)
                        || (heap.len() == limit && value >= heap.peek().map_or(0, |top| top.0))
                    {
                        return;
                    }
                    if accept(id) {
                        heap.push((value, id));
                        if heap.len() > limit {
                            heap.pop();
                        }
                    }
                });
                heap
            })
            .reduce(BinaryHeap::new, |mut left, right| {
                for item in right {
                    left.push(item);
                    if left.len() > limit {
                        left.pop();
                    }
                }
                left
            });
        let mut found = best.into_vec();
        found.sort_unstable();
        found
    }

    /// Visit a sequential order from its start. None when the budget runs out
    /// before the limit is filled or the spans are exhausted.
    fn walk(
        &self,
        spans: &[Span],
        order: Order,
        descending: bool,
        limit: usize,
        after: Option<Boundary>,
        accept: &(dyn Fn(u32) -> bool + Sync),
    ) -> Option<Vec<(u128, u32)>> {
        let mut ordered: Vec<Span> = spans.to_vec();
        ordered.sort_by_key(|span| span.bounds());
        if descending {
            ordered.reverse();
        }
        let mut found = Vec::with_capacity(limit.min(4096));
        let mut seen = 0usize;
        for span in ordered {
            let (mut low, mut high) = span.bounds();
            if let Some(boundary) = after {
                if descending {
                    high = high.min(boundary.key as usize);
                } else {
                    low = low.max(boundary.key as usize);
                }
            }
            let positions: Box<dyn Iterator<Item = usize>> = if descending {
                Box::new((low..high.max(low)).rev())
            } else {
                Box::new(low..high.max(low))
            };
            for position in positions {
                seen += 1;
                if seen > WALK_BUDGET {
                    return None;
                }
                let id = match span {
                    Span::Ids(..) => position as u32,
                    Span::Perm(..) => self.perm(position),
                };
                if accept(id) {
                    found.push((
                        Self::ordered(self.key(order, id, position), id, descending),
                        id,
                    ));
                    if found.len() == limit {
                        return Some(found);
                    }
                }
            }
        }
        Some(found)
    }
}
