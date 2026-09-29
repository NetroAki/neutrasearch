//! Resident index lifecycle for the MCP server: compact base + delta or the
//! legacy in-memory index, with reopen-on-replacement detection.

use anyhow::{bail, Context, Result};
 use neutra_core::{
     CompactIndex, DeltaIndex, DirectorySummaryEntry, Index, Query, SearchHit, SearchStats,
 };
use std::path::{Path, PathBuf};

pub(crate) enum Store {
    Compact {
        path: PathBuf,
        base: CompactIndex,
        delta: Option<Box<DeltaIndex>>,
    },
    Legacy {
        path: PathBuf,
        index: Index,
    },
}
impl Store {
    pub(crate) fn open(path: PathBuf) -> Result<Self> {
        if looks_compact(&path) {
            let (base, delta) = CompactIndex::open_with_delta_snapshot_fast(&path)
                .with_context(|| format!("open {}", path.display()))?;
            return Ok(Self::Compact {
                path,
                base,
                delta: delta.map(Box::new),
            });
        }

        let bytes = std::fs::read(&path)
            .with_context(|| format!("read configured index {}", path.display()))?;
        let index = Index::restore(&bytes)
            .with_context(|| format!("decode configured index {}", path.display()))?;
        Ok(Self::Legacy { path, index })
    }
    pub(crate) fn search(&mut self, q: &Query) -> Result<(Vec<SearchHit>, SearchStats)> {
        let reopen = match self {
            Self::Compact {
                path,
                base,
                delta: Some(delta),
            } => {
                CompactIndex::generation_on_disk(path)? != base.generation()
                    || delta.refresh().is_err()
            }
            Self::Compact {
                path,
                base,
                delta: None,
            } => {
                CompactIndex::generation_on_disk(path)? != base.generation()
                    || delta_path(path).is_file()
            }
            Self::Legacy { .. } => false,
        };
        if reopen {
            let path = self.path().to_path_buf();
            *self = Self::open(path).context("reopen compact index after replacement")?;
        }
        match self {
            Self::Compact {
                base,
                delta: Some(delta),
                ..
            } => Ok(base.search_with_delta(q, delta)?),
            Self::Compact {
                base, delta: None, ..
            } => Ok(base.search(q)?),
            Self::Legacy { index, .. } => index.search(q).map_err(anyhow::Error::from),
        }
    }
     /// One directory's live totals streamed from the base with the WAL
     /// overlay applied. `None` when the path is unindexed.
     pub(crate) fn directory_summary(
         &mut self,
         source: u32,
         path: &str,
     ) -> Result<Option<DirectorySummaryEntry>> {
         use neutra_core::{join_child_path, DirectoryChild, FileKind};
         let (base, delta) = match self {
             Self::Compact { base, delta, .. } => (base, delta),
             Self::Legacy { .. } => bail!("directory summaries require a compact index"),
         };
         let listing = base.list_directory(path, Some(source), delta.as_deref())?;
         if listing.total_count == 0 && listing.subdirs.is_empty() && listing.files.is_empty() {
             // An existing empty directory still answers; anything else here
             // was never indexed.
             if base.records_by_path(path)?.is_empty() {
                 return Ok(None);
             }
         }
         let mut children: Vec<DirectoryChild> = listing
             .subdirs
             .iter()
             .map(|child| DirectoryChild {
                 path: join_child_path(path, child.name.as_ref()).into(),
                 kind: FileKind::Dir,
                 logical_bytes: child.logical,
                 physical_bytes: child.size,
                 file_count: child.files,
                 directory_count: 0,
             })
             .collect();
         children.extend(listing.files.iter().map(|file| DirectoryChild {
             path: join_child_path(path, file.name.as_ref()).into(),
             kind: file.kind,
             logical_bytes: file.logical,
             physical_bytes: file.size,
             file_count: u64::from(matches!(
                 file.kind,
                 FileKind::File | FileKind::Symlink
             )),
             directory_count: 0,
         }));
         Ok(Some(DirectorySummaryEntry {
             source,
             path: path.into(),
             logical_bytes: listing.total_logical,
             physical_bytes: listing.total_size,
             file_count: listing.total_count,
             directory_count: listing.subdirs.len() as u64,
             children,
         }))
     }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Compact { path, .. } | Self::Legacy { path, .. } => path,
        }
    }
    pub(crate) fn len(&self) -> u64 {
        match self {
            Self::Compact { base, .. } => base.len(),
            Self::Legacy { index, .. } => index.len() as u64,
        }
    }
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Compact { delta: Some(_), .. } => "compact-mmap+delta",
            Self::Compact { delta: None, .. } => "compact-mmap",
            Self::Legacy { .. } => "legacy-resident",
        }
    }
    pub(crate) fn bytes(&self) -> u64 {
        match self {
            Self::Compact { base, delta, .. } => {
                base.mapped_bytes() as u64 + delta.as_ref().map_or(0, |delta| delta.wal_bytes())
            }
            Self::Legacy { path, .. } => std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
        }
    }
}

pub(crate) fn delta_path(base: &Path) -> PathBuf {
    let mut path = base.to_path_buf();
    path.set_extension("delta");
    path
}

pub(crate) fn looks_compact(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).is_ok() && &magic == b"NEUTIDX1"
}
