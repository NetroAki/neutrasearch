use super::*;

pub(crate) fn open_delta_writer(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::open_with_threshold(path, generation, threshold),
        None => DeltaIndex::open(path, generation),
    }
}

pub(crate) fn replace_empty_delta(
    path: &std::path::Path,
    generation: u64,
    compact_at: Option<u64>,
) -> std::io::Result<DeltaIndex> {
    match compact_at {
        Some(threshold) => DeltaIndex::replace_empty_with_threshold(path, generation, threshold),
        None => DeltaIndex::replace_empty(path, generation),
    }
}

pub(crate) fn retry_delta_with_staged_generation(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::UnexpectedEof
    )
}

pub(crate) fn recoverable_delta_error(error: &std::io::Error) -> bool {
    // A compaction reset can crash while rewriting the fixed-size WAL header.
    // Only that demonstrably incomplete state is recoverable. Complete but
    // invalid headers, frames, generations, and checksums must fail closed.
    error.kind() == std::io::ErrorKind::UnexpectedEof
}
