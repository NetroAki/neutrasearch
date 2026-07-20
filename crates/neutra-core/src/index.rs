//! In-memory merged index with parallel search.
//!
//! Records from all sources (local lanes + remote helpers) live in one
//! `Vec<FileRecord>`; searches are read-only parallel scans with rayon.
//! A million-record substring scan is single-digit milliseconds on a modern
//! CPU, which is why Everything-style tools keep it this simple.

use crate::query::{Query, SortKey};
use crate::types::FileRecord;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SearchStats {
    pub scanned: u64,
    pub matched: u64,
    pub wall_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub score: u32,
    /// Index into the records vec at search time.
    pub record: FileRecord,
}

#[derive(Default)]
pub struct Index {
    records: Vec<FileRecord>,
    /// Bumped on every mutation so clients can detect staleness.
    generation: u64,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn push(&mut self, rec: FileRecord) {
        self.records.push(rec);
    }

    /// Bulk append from a finished scan; one generation bump for the batch.
    pub fn extend(&mut self, recs: impl IntoIterator<Item = FileRecord>) {
        self.records.extend(recs);
        self.generation += 1;
    }

    /// Drop every record from a source (e.g. remote disconnected, or a
    /// mount about to be rescanned) and return how many were removed.
    pub fn remove_source(&mut self, source: u32) -> usize {
        let before = self.records.len();
        self.records.retain(|r| r.source != source);
        let removed = before - self.records.len();
        if removed > 0 {
            self.generation += 1;
        }
        removed
    }

    pub fn records(&self) -> &[FileRecord] {
        &self.records
    }

    pub fn into_records(self) -> Vec<FileRecord> {
        self.records
    }

    /// Parallel filter + score, then top-N by the query's sort key.
    pub fn search(&self, q: &Query) -> (Vec<SearchHit>, SearchStats) {
        let started = Instant::now();
        // Keep only each worker's top-N candidates. The matched count remains
        // exact, but an empty query over millions of records no longer builds
        // a millions-element temporary vector before truncation.
        let cmp = |a: &(u32, &FileRecord), b: &(u32, &FileRecord)| match q.sort {
            SortKey::Relevance => b.0.cmp(&a.0).then(b.1.mtime.cmp(&a.1.mtime)),
            SortKey::NameAsc => a.1.name().to_lowercase().cmp(&b.1.name().to_lowercase()),
            SortKey::PathAsc => a.1.path.cmp(&b.1.path),
            SortKey::SizeDesc => b.1.size.cmp(&a.1.size),
            SortKey::MtimeDesc => b.1.mtime.cmp(&a.1.mtime),
        };
        let prune = |ranked: &mut Vec<(u32, &FileRecord)>| {
            if q.limit > 0 && ranked.len() > q.limit {
                ranked.select_nth_unstable_by(q.limit, &cmp);
                ranked.truncate(q.limit);
            }
        };
        let (matched, mut ranked) = if q.limit > 0 {
            self.records
                .par_chunks(16_384)
                .map(|chunk| {
                    let mut matched = 0u64;
                    let mut ranked = Vec::with_capacity(q.limit.min(chunk.len()));
                    for record in chunk {
                        if q.passes_filters(record) {
                            if let Some(score) = q.score(record) {
                                matched += 1;
                                ranked.push((score, record));
                            }
                        }
                    }
                    prune(&mut ranked);
                    (matched, ranked)
                })
                .reduce(
                    || (0, Vec::new()),
                    |(left_count, mut left), (right_count, mut right)| {
                        left.append(&mut right);
                        prune(&mut left);
                        (left_count + right_count, left)
                    },
                )
        } else {
            let ranked = self
                .records
                .par_iter()
                .filter_map(|record| {
                    if !q.passes_filters(record) {
                        return None;
                    }
                    q.score(record).map(|score| (score, record))
                })
                .collect::<Vec<_>>();
            (ranked.len() as u64, ranked)
        };
        ranked.sort_unstable_by(&cmp);
        let hits = ranked
            .into_iter()
            .map(|(score, record)| SearchHit {
                score,
                record: record.clone(),
            })
            .collect();
        (
            hits,
            SearchStats {
                scanned: self.records.len() as u64,
                matched,
                wall_us: started.elapsed().as_micros() as u64,
            },
        )
    }

    /// Serialize the whole index (instant-restart cache on disk).
    pub fn snapshot(&self) -> bincode::Result<Vec<u8>> {
        bincode::serialize(&self.records)
    }

    pub fn restore(bytes: &[u8]) -> bincode::Result<Index> {
        let records: Vec<FileRecord> = bincode::deserialize(bytes)?;
        Ok(Index {
            records,
            generation: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mounts::FsKind;
    use crate::types::FileKind;

    fn rec(path: &str, size: u64, mtime: i64) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime,
            mode: 0,
            kind: FileKind::File,
            fs: FsKind::Btrfs,
            native_id: 0,
            native_parent: 0,
            source: 0,
        }
    }

    #[test]
    fn search_ranks_and_limits() {
        let mut idx = Index::new();
        idx.extend(vec![
            rec("/home/u/code/main.rs", 100, 5),
            rec("/home/u/doc/main-notes.txt", 100, 9),
            rec("/opt/main/lib.rs", 100, 1),
        ]);
        let q = Query::parse("main");
        let (hits, stats) = idx.search(&q);
        assert_eq!(stats.scanned, 3);
        assert_eq!(stats.matched, 3);
        assert_eq!(hits[0].record.path.as_ref(), "/home/u/doc/main-notes.txt");
        assert_eq!(hits[1].record.path.as_ref(), "/home/u/code/main.rs");
    }

    #[test]
    fn source_removal() {
        let mut idx = Index::new();
        idx.extend(vec![rec("/a", 1, 1), rec("/b", 1, 1)]);
        let mut remote = rec("/mnt/nas/c", 1, 1);
        remote.source = 7;
        idx.extend(vec![remote]);
        assert_eq!(idx.remove_source(7), 1);
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn snapshot_roundtrip() {
        let mut idx = Index::new();
        idx.extend(vec![rec("/a/b.rs", 42, 123)]);
        let bytes = idx.snapshot().unwrap();
        let idx2 = Index::restore(&bytes).unwrap();
        assert_eq!(idx2.len(), 1);
        assert_eq!(idx2.records()[0].path.as_ref(), "/a/b.rs");
    }
}
