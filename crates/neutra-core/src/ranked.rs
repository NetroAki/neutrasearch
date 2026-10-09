//! Precomputed leaders for "newest first" and "largest first". One scan per
//! index generation keeps the top of each ordering in a small sidecar; every
//! later listing reads that file and merges the live delta instead of decoding
//! the whole base. A listing the file cannot answer exactly returns `None`
//! and the caller runs the full search.

use crate::matcher::compare_records;
use crate::{CompactIndex, DeltaIndex, FileRecord, Query, SearchHit, SearchStats, SortKey};
use serde::{Deserialize, Serialize};
use std::collections::BinaryHeap;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAGIC: &[u8; 8] = b"NEURANK2";
const PREFIX: usize = 16;
/// Records kept per ordering. Enough that a kind filter still leaves a full page.
const KEEP: usize = 20_000;

struct RankedHit {
    sort: SortKey,
    score: u32,
    record: FileRecord,
}

impl PartialEq for RankedHit {
    fn eq(&self, other: &Self) -> bool {
        self.sort == other.sort
            && self.score == other.score
            && self.record.path == other.record.path
    }
}
impl Eq for RankedHit {}
impl PartialOrd for RankedHit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RankedHit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        compare_records(
            self.sort,
            &(self.score, &self.record),
            &(other.score, &other.record),
        )
    }
}

fn push_ranked(heap: &mut BinaryHeap<RankedHit>, hit: RankedHit, limit: usize) {
    if heap.len() < limit {
        heap.push(hit);
    } else if heap.peek().is_some_and(|worst| hit < *worst) {
        heap.pop();
        heap.push(hit);
    }
}

#[derive(Serialize, Deserialize)]
struct Lists {
    total: u64,
    newest: Vec<FileRecord>,
    largest: Vec<FileRecord>,
}

/// Index sidecars list every path, so they stay owner-only like the index.
fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(bytes)
}

pub struct RankedLists {
    lists: Lists,
}

impl RankedLists {
    pub fn path_for(index_path: &Path) -> PathBuf {
        let mut value = index_path.as_os_str().to_os_string();
        value.push(".rank");
        value.into()
    }

    fn header_generation(bytes: &[u8]) -> io::Result<u64> {
        if bytes.len() < PREFIX || &bytes[..8] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a ranked list",
            ));
        }
        Ok(u64::from_le_bytes(bytes[8..16].try_into().unwrap()))
    }

    /// Whether the file on disk was built for `generation`.
    pub fn is_current(index_path: &Path, generation: u64) -> bool {
        let found = std::fs::File::open(Self::path_for(index_path)).and_then(|mut file| {
            let mut header = [0u8; PREFIX];
            file.read_exact(&mut header)?;
            Self::header_generation(&header)
        });
        found.is_ok_and(|found| found == generation)
    }

    /// Build the lists for `generation` unless a current file is on disk.
    pub fn ensure(index_path: &Path, generation: u64) -> io::Result<()> {
        if Self::is_current(index_path, generation) {
            return Ok(());
        }
        let index = CompactIndex::open_fast(index_path)?;
        if index.generation() != generation {
            return Err(io::Error::other("index replaced while ranking"));
        }
        let top = |sort| -> io::Result<Vec<FileRecord>> {
            let query = Query {
                sort,
                limit: KEEP,
                ..Query::default()
            };
            Ok(index
                .search(&query)?
                .0
                .into_iter()
                .map(|hit| hit.record)
                .collect())
        };
        let lists = Lists {
            total: index.len(),
            newest: top(SortKey::MtimeDesc)?,
            largest: top(SortKey::SizeDesc)?,
        };
        let body = bincode::serialize(&lists).map_err(io::Error::other)?;
        let mut out = Vec::with_capacity(PREFIX + body.len() / 2);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&generation.to_le_bytes());
        out.extend_from_slice(&zstd::stream::encode_all(&body[..], 3)?);
        let tmp = Self::path_for(index_path).with_extension("rank.tmp");
        write_private(&tmp, &out)?;
        std::fs::rename(tmp, Self::path_for(index_path))
    }

    pub fn open_for_compact(index_path: &Path, generation: u64) -> io::Result<Self> {
        let bytes = std::fs::read(Self::path_for(index_path))?;
        if Self::header_generation(&bytes)? != generation {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "ranked list is stale",
            ));
        }
        let raw = zstd::stream::decode_all(&bytes[PREFIX..])?;
        let lists = bincode::deserialize(&raw)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Self { lists })
    }

    /// Answer a term-free listing sorted by modified time or size, or `None`
    /// when the stored leaders cannot prove the page is exact.
    pub fn search(
        &self,
        q: &Query,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<Option<(Vec<SearchHit>, SearchStats)>> {
        if !q.terms.is_empty() || q.regex.is_some() || q.limit == 0 || q.limit > KEEP {
            return Ok(None);
        }
        let base = match q.sort {
            SortKey::MtimeDesc => &self.lists.newest,
            SortKey::SizeDesc => &self.lists.largest,
            _ => return Ok(None),
        };
        let started = Instant::now();
        let matcher = q.matcher()?;
        let mut kept = BinaryHeap::<RankedHit>::with_capacity(q.limit.min(10_000));
        let mut from_base = 0usize;
        let mut shadowed = 0usize;
        for record in base {
            let is_shadowed = match delta {
                Some(d) => d.shadows(record.path.as_ref())?,
                None => false,
            };
            if is_shadowed {
                shadowed += 1;
                continue;
            }
            if !q.passes_filters(record) {
                continue;
            }
            if let Some(score) = matcher.score(record) {
                push_ranked(
                    &mut kept,
                    RankedHit {
                        sort: q.sort,
                        score,
                        record: record.clone(),
                    },
                    q.limit,
                );
                from_base += 1;
            }
        }
        // A full page of surviving leaders is provably the true top: every
        // record outside the file ranks below all of them.
        if from_base < q.limit && self.lists.total > base.len() as u64 {
            return Ok(None);
        }
        let mut added = 0u64;
        if let Some(delta) = delta {
            delta.for_each_upsert(|record| {
                if q.passes_filters(&record) {
                    if let Some(score) = matcher.score(&record) {
                        push_ranked(
                            &mut kept,
                            RankedHit {
                                sort: q.sort,
                                score,
                                record,
                            },
                            q.limit,
                        );
                        added += 1;
                    }
                }
                Ok(())
            })?;
        }
        let mut kept = kept.into_vec();
        kept.sort_unstable_by(|a, b| {
            compare_records(q.sort, &(a.score, &a.record), &(b.score, &b.record))
        });
        let unfiltered = q.kinds.is_empty()
            && q.exts.is_empty()
            && q.fss.is_empty()
            && q.min_size.is_none()
            && q.max_size.is_none()
            && q.under.is_none()
            && q.scope_roots.is_empty()
            && !q.executable_only
            && q.exclude_roots.is_empty();
        // Nothing filtered out of the leaders means the index total is the
        // best count available; otherwise only a lower bound is known.
        let matched = if unfiltered || from_base + shadowed == base.len() {
            self.lists.total + added
        } else {
            from_base as u64 + added
        };
        let hits = kept
            .into_iter()
            .map(|hit| SearchHit {
                score: hit.score,
                record: hit.record,
            })
            .collect();
        let stats = SearchStats {
            scanned: self.lists.total + added,
            matched,
            wall_us: started.elapsed().as_micros() as u64,
        };
        Ok(Some((hits, stats)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FsKind;

    fn rec(path: &str, size: u64, mtime: i64, kind: crate::FileKind) -> FileRecord {
        FileRecord {
            path: path.into(),
            size,
            mtime,
            mode: 0,
            kind,
            fs: FsKind::Btrfs,
            native_id: size + 1,
            native_parent: 0,
            source: 0,
            disk: size,
        }
    }

    #[test]
    fn leaders_match_the_full_search_for_both_orderings_and_filters() {
        let records = vec![
            rec("/a/old.txt", 5, 100, crate::FileKind::File),
            rec("/a/new.wav", 900, 900, crate::FileKind::File),
            rec("/a/mid.txt", 70, 500, crate::FileKind::File),
            rec("/a/dir", 0, 700, crate::FileKind::Dir),
            rec("/b/big.bin", 5000, 300, crate::FileKind::File),
        ];
        let path = std::env::temp_dir().join(format!("neutra-rank-{}.idx", std::process::id()));
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open_fast(&path).unwrap();
        let generation = index.generation();
        assert!(!RankedLists::is_current(&path, generation));
        RankedLists::ensure(&path, generation).unwrap();
        assert!(RankedLists::is_current(&path, generation));
        let ranked = RankedLists::open_for_compact(&path, generation).unwrap();
        let names = |hits: &[SearchHit]| {
            hits.iter()
                .map(|h| h.record.path.to_string())
                .collect::<Vec<_>>()
        };
        for sort in [SortKey::MtimeDesc, SortKey::SizeDesc] {
            for kinds in [vec![], vec![crate::FileKind::File]] {
                let query = Query {
                    sort,
                    limit: 3,
                    kinds,
                    ..Query::default()
                };
                let want = index.search(&query).unwrap().0;
                let got = ranked.search(&query, None).unwrap().expect("answerable").0;
                assert_eq!(names(&got), names(&want), "{sort:?}");
            }
        }
        let typed = Query {
            terms: vec!["old".into()],
            sort: SortKey::MtimeDesc,
            limit: 3,
            ..Query::default()
        };
        assert!(ranked.search(&typed, None).unwrap().is_none());
        let by_name = Query {
            sort: SortKey::NameAsc,
            limit: 3,
            ..Query::default()
        };
        assert!(ranked.search(&by_name, None).unwrap().is_none());
        drop(index);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(RankedLists::path_for(&path));
    }

    #[test]
    fn a_saved_file_leads_the_newest_listing_and_a_removed_one_leaves_it() {
        let records = vec![
            rec("/a/one.txt", 5, 100, crate::FileKind::File),
            rec("/a/two.txt", 6, 200, crate::FileKind::File),
            rec("/a/three.txt", 7, 300, crate::FileKind::File),
        ];
        let path =
            std::env::temp_dir().join(format!("neutra-rank-delta-{}.idx", std::process::id()));
        CompactIndex::build(&records, &path).unwrap();
        let generation = CompactIndex::open_fast(&path).unwrap().generation();
        RankedLists::ensure(&path, generation).unwrap();
        let ranked = RankedLists::open_for_compact(&path, generation).unwrap();
        let delta_path = path.with_extension("delta");
        let mut delta = DeltaIndex::open(&delta_path, generation).unwrap();
        delta
            .apply(crate::DeltaChange::Upsert(rec(
                "/a/saved.txt",
                9,
                999,
                crate::FileKind::File,
            )))
            .unwrap();
        delta
            .apply(crate::DeltaChange::Remove("/a/three.txt".into()))
            .unwrap();
        let query = Query {
            sort: SortKey::MtimeDesc,
            limit: 10,
            ..Query::default()
        };
        let hits = ranked
            .search(&query, Some(&delta))
            .unwrap()
            .expect("answerable")
            .0;
        let paths: Vec<_> = hits.iter().map(|h| h.record.path.to_string()).collect();
        assert_eq!(paths, ["/a/saved.txt", "/a/two.txt", "/a/one.txt"]);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&delta_path);
        let _ = std::fs::remove_file(RankedLists::path_for(&path));
    }

    #[test]
    fn corrupt_future_timestamps_do_not_lead_the_newest_listing() {
        let records = vec![
            rec("/a/real.txt", 5, 1_790_000_000, crate::FileKind::File),
            rec("/a/bogus.txt", 5, 4_500_000_000, crate::FileKind::File),
        ];
        let path =
            std::env::temp_dir().join(format!("neutra-rank-future-{}.idx", std::process::id()));
        CompactIndex::build(&records, &path).unwrap();
        let index = CompactIndex::open_fast(&path).unwrap();
        let query = Query {
            sort: SortKey::MtimeDesc,
            limit: 5,
            ..Query::default()
        };
        assert_eq!(
            &*index.search(&query).unwrap().0[0].record.path,
            "/a/real.txt"
        );
        drop(index);
        let _ = std::fs::remove_file(&path);
    }
}
