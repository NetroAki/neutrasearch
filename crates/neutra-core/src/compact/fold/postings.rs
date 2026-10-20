//! Trigram postings for a folded base: the old lists renumbered for the
//! blocks that were copied, merged with the pairs of the new blocks.

use super::*;

/// Every (gram, block) pair of the old index whose block survived, with the
/// block renumbered. Ascending, because renumbering is monotonic.
pub(super) fn carried_pairs<'a>(
    base: &'a CompactIndex,
    old_to_new: &'a [u32],
) -> impl Iterator<Item = io::Result<u64>> + 'a {
    let mut next = 0usize;
    let mut pending: std::vec::IntoIter<u64> = Vec::new().into_iter();
    std::iter::from_fn(move || loop {
        if let Some(pair) = pending.next() {
            return Some(Ok(pair));
        }
        if next >= base.dict.count {
            return None;
        }
        let Some(entry) = base.dict_entry(next) else {
            return Some(Err(invalid("trigram dictionary entry out of range")));
        };
        next += 1;
        match base.decode_posting(entry) {
            Ok(ids) => {
                let gram = u64::from(entry.gram) << 32;
                pending = ids
                    .into_iter()
                    .filter_map(|id| {
                        let new = *old_to_new.get(id as usize)?;
                        (new != u32::MAX).then_some(gram | u64::from(new))
                    })
                    .collect::<Vec<_>>()
                    .into_iter();
            }
            Err(error) => return Some(Err(error)),
        }
    })
}

type Pairs<'a> = Box<dyn Iterator<Item = io::Result<u64>> + 'a>;

/// Ordered union of two ascending pair streams.
pub(super) struct Merged<'a> {
    left: Pairs<'a>,
    right: Pairs<'a>,
    left_head: Option<u64>,
    right_head: Option<u64>,
    started: bool,
}

impl<'a> Merged<'a> {
    pub(super) fn new(
        left: impl Iterator<Item = io::Result<u64>> + 'a,
        right: impl Iterator<Item = io::Result<u64>> + 'a,
    ) -> Self {
        Self {
            left: Box::new(left),
            right: Box::new(right),
            left_head: None,
            right_head: None,
            started: false,
        }
    }
}

impl Iterator for Merged<'_> {
    type Item = io::Result<u64>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.started {
            self.started = true;
            match (self.left.next().transpose(), self.right.next().transpose()) {
                (Ok(left), Ok(right)) => (self.left_head, self.right_head) = (left, right),
                (Err(error), _) | (_, Err(error)) => return Some(Err(error)),
            }
        }
        let take_left = match (self.left_head, self.right_head) {
            (None, None) => return None,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (Some(left), Some(right)) => left <= right,
        };
        let (value, source) = if take_left {
            (self.left_head, &mut self.left)
        } else {
            (self.right_head, &mut self.right)
        };
        let advanced = match source.next().transpose() {
            Ok(next) => next,
            Err(error) => return Some(Err(error)),
        };
        if take_left {
            self.left_head = advanced;
        } else {
            self.right_head = advanced;
        }
        value.map(Ok)
    }
}
