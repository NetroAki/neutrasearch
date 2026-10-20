use super::*;

pub(super) fn emit_records<F>(records: &[FileRecord], order: &[u32], mut emit: F) -> io::Result<()>
where
    F: FnMut(DirectorySummaryEntry) -> io::Result<()>,
{
    let mut stack = Vec::<OpenEntry>::new();
    for (position, index) in order.iter().enumerate() {
        let record = &records[*index as usize];
        let normalized_path = normalize_path(record.path.as_ref())?;
        if position > 0 {
            let previous = &records[order[position - 1] as usize];
            if previous.source == record.source
                && compare_paths(previous.path.as_ref(), &normalized_path) == Ordering::Equal
            {
                // Native lanes can expose aliases, but an exact source/path
                // pair must contribute once to prevent duplicate totals.
                continue;
            }
        }
        let ancestors = ancestor_paths(&normalized_path);
        let desired = ancestors
            .get(..ancestors.len().saturating_sub(1))
            .unwrap_or_default();
        let common = stack
            .iter()
            .zip(desired)
            .take_while(|(entry, path)| entry.source == record.source && entry.path == **path)
            .count();
        close_stack(&mut stack, common, &mut emit)?;
        for path in &desired[common..] {
            open_entry(&mut stack, record.source, path);
        }

        if record.kind == FileKind::Dir {
            for entry in &mut stack {
                entry.directory_count = entry.directory_count.saturating_add(1);
            }
            open_entry(&mut stack, record.source, &normalized_path);
            continue;
        }

        let contributes_file = matches!(record.kind, FileKind::File | FileKind::Symlink);
        for entry in &mut stack {
            if contributes_file {
                entry.logical_bytes = entry.logical_bytes.saturating_add(record.size);
                entry.physical_bytes = entry.physical_bytes.saturating_add(record.disk_bytes());
                entry.file_count = entry.file_count.saturating_add(1);
            }
        }
        if let Some(parent) = stack.last_mut() {
            parent.children.push(DirectoryChild {
                path: normalized_path.clone().into_boxed_str(),
                kind: record.kind,
                logical_bytes: record.size,
                physical_bytes: record.disk_bytes(),
                file_count: u64::from(contributes_file),
                directory_count: 0,
            });
        }
    }
    close_stack(&mut stack, 0, &mut emit)
}

pub(crate) struct OpenEntry {
    pub(crate) source: u32,
    pub(crate) path: String,
    pub(crate) logical_bytes: u64,
    pub(crate) physical_bytes: u64,
    pub(crate) file_count: u64,
    pub(crate) directory_count: u64,
    pub(crate) children: Vec<DirectoryChild>,
}

impl OpenEntry {
    fn finish(mut self) -> DirectorySummaryEntry {
        self.children.sort_unstable_by(|left, right| {
            compare_paths(&left.path, &right.path)
                .then_with(|| left.kind_rank().cmp(&right.kind_rank()))
        });
        DirectorySummaryEntry {
            source: self.source,
            path: self.path.into_boxed_str(),
            logical_bytes: self.logical_bytes,
            physical_bytes: self.physical_bytes,
            file_count: self.file_count,
            directory_count: self.directory_count,
            children: self.children,
        }
    }
}

pub(crate) fn open_entry(stack: &mut Vec<OpenEntry>, source: u32, path: &str) {
    if let Some(parent) = stack.last_mut() {
        parent.children.push(DirectoryChild {
            path: path.to_owned().into_boxed_str(),
            kind: FileKind::Dir,
            logical_bytes: 0,
            physical_bytes: 0,
            file_count: 0,
            directory_count: 0,
        });
    }
    stack.push(OpenEntry {
        source,
        path: path.to_owned(),
        logical_bytes: 0,
        physical_bytes: 0,
        file_count: 0,
        directory_count: 0,
        children: Vec::new(),
    });
}

pub(crate) fn close_stack<F>(
    stack: &mut Vec<OpenEntry>,
    keep: usize,
    emit: &mut F,
) -> io::Result<()>
where
    F: FnMut(DirectorySummaryEntry) -> io::Result<()>,
{
    while stack.len() > keep {
        let finished = stack.pop().expect("stack length checked").finish();
        if let Some(parent) = stack.last_mut() {
            if let Some(child) = parent.children.iter_mut().rev().find(|child| {
                child.kind == FileKind::Dir && child.path.as_ref() == finished.path.as_ref()
            }) {
                child.logical_bytes = finished.logical_bytes;
                child.physical_bytes = finished.physical_bytes;
                child.file_count = finished.file_count;
                child.directory_count = finished.directory_count;
            }
        }
        emit(finished)?;
    }
    Ok(())
}

pub(crate) fn summary_order(records: &[FileRecord]) -> io::Result<Vec<u32>> {
    let mut order = (0..records.len())
        .map(|index| {
            u32::try_from(index).map_err(|_| invalid("too many records for summary order"))
        })
        .collect::<io::Result<Vec<_>>>()?;
    order.par_sort_by(|left, right| {
        records[*left as usize]
            .source
            .cmp(&records[*right as usize].source)
            .then_with(|| {
                compare_paths(
                    records[*left as usize].path.as_ref(),
                    records[*right as usize].path.as_ref(),
                )
            })
    });
    Ok(order)
}

/// Aggregate records into summary entries in file order, unsorted. The tree
/// builds straight from these; callers needing binary search sort them.
pub fn aggregate_records(records: &[FileRecord]) -> io::Result<Vec<DirectorySummaryEntry>> {
    let order = summary_order(records)?;
    let mut entries = Vec::new();
    emit_records(records, &order, |entry| {
        entries.push(entry);
        Ok(())
    })?;
    Ok(entries)
}
