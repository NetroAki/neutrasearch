//! Column-level filters that never reject a row the full filter accepts.
use super::{BlockReader, Columns, OTHER_EXT, SIZE_MAX};
use crate::Query;
use std::io;

/// Cheap column checks. They may accept rows the full filter later rejects,
/// but never reject a row the full filter accepts.
pub(in crate::browse) struct Prefilter {
    kinds: u8,
    fs: u16,
    executable: bool,
    exts: Option<Vec<u16>>,
    min_size: Option<u64>,
    max_size: Option<u64>,
    include: Option<Vec<(u32, u32)>>,
    exclude: Vec<(u32, u32)>,
}

impl Prefilter {
    pub(in crate::browse) fn new(
        cols: &Columns,
        reader: &mut BlockReader<'_>,
        query: &Query,
    ) -> io::Result<Self> {
        let mut kinds = 0u8;
        for kind in &query.kinds {
            kinds |= 1 << super::kind_code(*kind);
        }
        let mut fs = 0u16;
        for filesystem in &query.fss {
            fs |= 1 << super::fs_code(filesystem);
        }
        let exts = (!query.exts.is_empty()).then(|| {
            let mut ids: Vec<u16> = query
                .exts
                .iter()
                .filter_map(|ext| cols.ext_id(&ext.to_ascii_lowercase()))
                .collect();
            if cols.ext_overflow {
                ids.push(OTHER_EXT);
            }
            ids
        });
        let include = if query.scope_roots.is_empty() {
            None
        } else {
            scope_ranges(reader, &query.scope_roots, query.scope_case_sensitive)?
        };
        let mut exclude = Vec::new();
        for root in &query.exclude_roots {
            if let Some(found) = scope_ranges(
                reader,
                std::slice::from_ref(root),
                query.scope_case_sensitive,
            )? {
                exclude.extend(found);
            }
        }
        Ok(Self {
            kinds: if kinds == 0 { 0xF } else { kinds },
            fs: if fs == 0 { u16::MAX } else { fs },
            executable: query.executable_only,
            exts,
            min_size: query.min_size,
            max_size: query.max_size,
            include,
            exclude: merge(exclude),
        })
    }

    pub(in crate::browse) fn accepts(&self, cols: &Columns, id: u32) -> bool {
        let attrs = cols.attrs(id);
        if self.kinds & (1 << ((attrs >> 20) & 7)) == 0
            || self.fs & (1 << ((attrs >> 16) & 15)) == 0
            || (self.executable && attrs >> 23 & 1 == 0)
        {
            return false;
        }
        if let Some(exts) = &self.exts {
            if !exts.contains(&((attrs & 0xFFFF) as u16)) {
                return false;
            }
        }
        if self.min_size.is_some() || self.max_size.is_some() {
            let size = cols.size(id);
            if size != SIZE_MAX
                && (self.min_size.is_some_and(|min| size < min)
                    || self.max_size.is_some_and(|max| size > max))
            {
                return false;
            }
        }
        if let Some(include) = &self.include {
            let at = include.partition_point(|&(low, _)| low <= id);
            if at == 0 || id >= include[at - 1].1 {
                return false;
            }
        }
        !self
            .exclude
            .iter()
            .any(|&(low, high)| low <= id && id < high)
    }

    /// Id intervals to visit, or the whole base when no scope applies.
    pub(in crate::browse) fn spans(&self, len: usize) -> Vec<(u32, u32)> {
        self.include
            .clone()
            .unwrap_or_else(|| vec![(0, len as u32)])
    }
}

fn merge(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.retain(|(low, high)| low < high);
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (low, high) in ranges {
        match merged.last_mut() {
            Some(last) if low <= last.1 => last.1 = last.1.max(high),
            _ => merged.push((low, high)),
        }
    }
    merged
}

/// Id intervals covering the roots, or None when a root cannot be expressed
/// as a byte-ordered path interval (so the caller falls back to full checks).
pub(in crate::browse) fn scope_ranges(
    reader: &mut BlockReader<'_>,
    roots: &[String],
    case_sensitive: bool,
) -> io::Result<Option<Vec<(u32, u32)>>> {
    let mut ranges = Vec::new();
    for root in roots {
        if root == "/" {
            return Ok(None);
        }
        if !case_sensitive || !root.starts_with('/') || root.contains('\\') {
            return Ok(None);
        }
        let trimmed = root.trim_end_matches('/');
        let exact = reader.path_lower_bound(trimmed)?;
        let after = reader.path_lower_bound(&format!("{trimmed}\0"))?;
        let low = reader.path_lower_bound(&format!("{trimmed}/"))?;
        let high = reader.path_lower_bound(&format!("{trimmed}0"))?;
        ranges.push((exact, after));
        ranges.push((low, high));
    }
    Ok(Some(merge(ranges)))
}
