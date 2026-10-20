use super::*;
mod batch;

pub(super) fn collect(
    index: &CompactIndex,
    blocks: &[u32],
    matcher: &crate::matcher::QueryMatcher,
    delta: Option<&DeltaIndex>,
    after: Option<(u32, &FileRecord)>,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> io::Result<(u64, Vec<(u32, FileRecord)>)> {
    let q = matcher.query();
    let cmp = |a: &(u32, FileRecord), b: &(u32, FileRecord)| {
        compare_records(q.sort, &(a.0, &a.1), &(b.0, &b.1))
    };
    let prune_at = q.limit.saturating_mul(2).max(q.limit.saturating_add(32));
    let mut matched = 0u64;
    let mut ranked = Vec::new();
    let mut pending = Vec::with_capacity(512);
    let mut scratch = scan::BlockScratch::new()?;
    for &block in blocks {
        if cancelled() {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "search superseded",
            ));
        }
        index.scan_block(block, &mut scratch, &mut |view| {
            pending.extend(CompactIndex::view_hit(matcher, view));
        })?;
        if pending.len() < 512 {
            continue;
        }
        matched += batch::merge(&mut pending, &mut ranked, q.sort, delta, after)?;
        if q.limit > 0 && ranked.len() >= prune_at {
            retain_best(&mut ranked, q.limit, &cmp);
        }
    }
    matched += batch::merge(&mut pending, &mut ranked, q.sort, delta, after)?;
    if q.limit > 0 && ranked.len() > q.limit {
        retain_best(&mut ranked, q.limit, &cmp);
    }
    Ok((matched, ranked))
}
