use super::*;

pub(super) fn merge(
    pending: &mut Vec<(u32, FileRecord)>,
    ranked: &mut Vec<(u32, FileRecord)>,
    sort: crate::SortKey,
    delta: Option<&DeltaIndex>,
    after: Option<(u32, &FileRecord)>,
) -> io::Result<u64> {
    if pending.is_empty() {
        return Ok(0);
    }
    // Check only matching records, in bounded batches rather than one SQL lookup per tiny block.
    let shadows = match delta {
        Some(overlay) => overlay.shadows_batch(
            &pending
                .iter()
                .map(|(_, record)| record.path.as_ref())
                .collect::<Vec<_>>(),
        )?,
        None => vec![false; pending.len()],
    };
    let mut matched = 0;
    for ((score, record), shadowed) in pending.drain(..).zip(shadows) {
        if !shadowed {
            matched += 1;
            if past_cursor(sort, score, &record, after) {
                ranked.push((score, record));
            }
        }
    }
    Ok(matched)
}
