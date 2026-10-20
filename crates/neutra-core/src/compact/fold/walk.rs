//! Ordered walk over the old blocks and the sorted delta upserts. Untouched
//! blocks are copied; blocks the delta shadows or lands inside are merged
//! record by record.

use super::output::Output;
use super::*;
use crate::compact_merge::RunMerge;

/// Blocks decoded together; large enough to keep every core busy.
const BATCH: u32 = 1024;

struct Upserts {
    merge: RunMerge,
    head: Option<FileRecord>,
}

impl Upserts {
    /// Emit upserts ordered before `limit` (or equal to it when `inclusive`).
    fn emit_until(&mut self, out: &mut Output, limit: &str, inclusive: bool) -> io::Result<()> {
        while let Some(head) = &self.head {
            let order = compare_index_paths(head.path.as_ref(), limit);
            if order == Ordering::Greater || (order == Ordering::Equal && !inclusive) {
                break;
            }
            let record = self.head.take().expect("head checked");
            out.push(record)?;
            self.head = self.merge.next().transpose()?;
        }
        Ok(())
    }

    fn reaches(&self, limit: &str) -> bool {
        self.head
            .as_ref()
            .is_some_and(|head| compare_index_paths(head.path.as_ref(), limit) != Ordering::Greater)
    }
}

pub(super) fn run(
    base: &CompactIndex,
    delta: &DeltaIndex,
    mut merge: RunMerge,
    out: &mut Output,
) -> io::Result<()> {
    let head = merge.next().transpose()?;
    let mut upserts = Upserts { merge, head };
    let total = base.blocks.count as u32;
    let mut first_id = 0u32;
    while first_id < total {
        let end = (first_id + BATCH).min(total);
        let decoded: Vec<io::Result<(Vec<FileRecord>, Vec<bool>)>> = (first_id..end)
            .into_par_iter()
            .map(|id| {
                let records = base.read_block(id)?;
                let paths: Vec<&str> = records.iter().map(|r| r.path.as_ref()).collect();
                let shadowed = delta.shadows_batch(&paths)?;
                Ok((records, shadowed))
            })
            .collect();
        for (offset, item) in decoded.into_iter().enumerate() {
            let (records, shadowed) = item?;
            let id = first_id + offset as u32;
            let (Some(first), Some(last)) = (records.first(), records.last()) else {
                continue;
            };
            let (first, last) = (first.path.clone(), last.path.clone());
            upserts.emit_until(out, &first, false)?;
            if !upserts.reaches(&last) && !shadowed.contains(&true) {
                out.copy(base, id, records)?;
                continue;
            }
            for (record, hidden) in records.into_iter().zip(shadowed) {
                if hidden {
                    continue;
                }
                upserts.emit_until(out, &record.path, false)?;
                out.push(record)?;
            }
            upserts.emit_until(out, &last, true)?;
        }
        first_id = end;
    }
    while let Some(record) = upserts.head.take() {
        out.push(record)?;
        upserts.head = upserts.merge.next().transpose()?;
    }
    out.flush()
}
