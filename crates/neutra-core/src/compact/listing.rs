use super::*;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

struct RankedFile(DirFile);
impl PartialEq for RankedFile {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for RankedFile {}
impl PartialOrd for RankedFile {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for RankedFile {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .size
            .cmp(&other.0.size)
            .then_with(|| other.0.name.cmp(&self.0.name))
    }
}

struct Listing {
    result: DirListing,
    subdirs: HashMap<Box<str>, (u64, u64, u64)>,
    files: BinaryHeap<Reverse<RankedFile>>,
    limit: usize,
}

impl Listing {
    fn new(limit: usize) -> Self {
        Self {
            result: DirListing::default(),
            subdirs: HashMap::new(),
            files: BinaryHeap::new(),
            limit,
        }
    }

    fn absorb(&mut self, record: &FileRecord, prefix: &str) {
        let relative = &record.path.as_ref()[prefix.len()..];
        if relative.is_empty() {
            return;
        }
        let contributes = matches!(
            record.kind,
            crate::FileKind::File | crate::FileKind::Symlink
        );
        let size = if contributes { record.disk_bytes() } else { 0 };
        let logical = if contributes { record.size } else { 0 };
        let count = u64::from(contributes);
        if let Some(slash) = relative.find('/') {
            let bucket = self.subdirs.entry(relative[..slash].into()).or_default();
            bucket.0 = bucket.0.saturating_add(size);
            bucket.1 = bucket.1.saturating_add(logical);
            bucket.2 = bucket.2.saturating_add(count);
        } else if record.kind == crate::FileKind::Dir {
            self.subdirs.entry(relative.into()).or_default();
            return;
        } else {
            self.file(relative, record, size, logical);
        }
        self.result.total_size = self.result.total_size.saturating_add(size);
        self.result.total_logical = self.result.total_logical.saturating_add(logical);
        self.result.total_count = self.result.total_count.saturating_add(count);
    }

    fn file(&mut self, name: &str, record: &FileRecord, size: u64, logical: u64) {
        if self.limit == 0 {
            self.result.files_truncated = true;
            return;
        }
        if self.files.len() == self.limit {
            self.result.files_truncated = true;
            let worst = &self.files.peek().unwrap().0 .0;
            if size < worst.size || (size == worst.size && name >= &*worst.name) {
                return;
            }
            self.files.pop();
        }
        self.files.push(Reverse(RankedFile(DirFile {
            name: name.into(),
            size,
            logical,
            kind: record.kind,
        })));
    }

    fn finish(mut self) -> DirListing {
        self.result.files = self
            .files
            .into_sorted_vec()
            .into_iter()
            .map(|file| file.0 .0)
            .collect();
        self.result.subdirs = self
            .subdirs
            .into_iter()
            .map(|(name, (size, logical, files))| DirSubdir {
                name,
                size,
                logical,
                files,
            })
            .collect();
        self.result
            .subdirs
            .sort_unstable_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
        self.result
    }
}

impl CompactIndex {
    pub fn list_directory(
        &self,
        dir: &str,
        source: Option<u32>,
        delta: Option<&DeltaIndex>,
    ) -> io::Result<DirListing> {
        self.list_directory_with_limit(dir, source, delta, Self::MAX_LISTED_FILES)
    }

    pub fn list_directory_with_limit(
        &self,
        dir: &str,
        source: Option<u32>,
        delta: Option<&DeltaIndex>,
        limit: usize,
    ) -> io::Result<DirListing> {
        let prefix = dir_prefix(dir);
        let legacy = !cfg!(windows) && self.record_version < 7;
        let low = if legacy {
            0
        } else {
            self.first_directory_block(&prefix)?
        };
        let mut listing = Listing::new(limit.min(Self::MAX_LISTED_FILES));
        let mut previous = if low > 0 {
            self.read_block(low as u32 - 1)?.pop()
        } else {
            None
        };
        if low > 0 {
            self.release_block(low as u32 - 1);
        }
        for block_id in low..self.blocks.count {
            let records = self.read_block(block_id as u32)?;
            let paths: Vec<_> = records.iter().map(|record| record.path.as_ref()).collect();
            let shadows = match delta {
                Some(delta) => delta.shadows_batch(&paths)?,
                None => vec![false; paths.len()],
            };
            let mut past = false;
            for (record, shadowed) in records.into_iter().zip(shadows) {
                if source.is_some_and(|wanted| record.source != wanted) {
                    continue;
                }
                let duplicate = previous.as_ref().is_some_and(|prior: &FileRecord| {
                    prior.source == record.source && folded_eq(&prior.path, &record.path)
                });
                if !duplicate && !shadowed && starts_with_folded(&record.path, &prefix) {
                    listing.absorb(&record, &prefix);
                }
                if !legacy
                    && compare_index_paths(&record.path, &prefix) == Ordering::Greater
                    && !starts_with_folded(&record.path, &prefix)
                {
                    past = true;
                    break;
                }
                previous = Some(record);
            }
            self.release_block(block_id as u32);
            if past {
                break;
            }
        }
        if let Some(delta) = delta {
            delta.for_each_upsert(|record| {
                if source.is_none_or(|wanted| record.source == wanted)
                    && starts_with_folded(&record.path, &prefix)
                {
                    listing.absorb(&record, &prefix);
                }
                Ok(())
            })?;
        }
        Ok(listing.finish())
    }

    fn first_directory_block(&self, prefix: &str) -> io::Result<usize> {
        let (mut low, mut high) = (0, self.blocks.count);
        while low < high {
            let middle = low + (high - low) / 2;
            let records = self.read_block(middle as u32)?;
            self.release_block(middle as u32);
            if records
                .last()
                .is_some_and(|record| compare_index_paths(&record.path, prefix) == Ordering::Less)
            {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }
}
