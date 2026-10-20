use super::*;

pub(super) enum Kind<'a> {
    Folder { size: u64, count: u64 },
    File(&'a TreeFile),
    More(usize),
    Loading,
}

pub(super) struct Row<'a> {
    pub(super) path: &'a str,
    pub(super) depth: usize,
    pub(super) share: f32,
    pub(super) kind: Kind<'a>,
}

pub(super) enum Item<'a> {
    Dir {
        path: &'a str,
        depth: usize,
        size: u64,
        count: u64,
        parent: u64,
    },
    Files {
        path: &'a str,
        depth: usize,
    },
}

pub(super) fn share(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).min(1.0) as f32
    }
}

pub(super) fn short_count(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=999_999 => format!("{:.1}K", count as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", count as f64 / 1e6),
        _ => format!("{:.1}B", count as f64 / 1e9),
    }
}

/// Visible rows in display order: each expanded folder lists its subfolders
/// and then, for the open folder only, its largest files.
pub(super) fn flatten<'a>(
    hierarchy: &'a Hierarchy,
    current: &str,
    expanded: &BTreeSet<String>,
) -> Vec<Row<'a>> {
    let root = hierarchy.folders.get("/").cloned().unwrap_or_default();
    let mut rows = Vec::new();
    let mut stack = vec![Item::Dir {
        path: "/",
        depth: 0,
        size: root.size,
        count: root.count,
        parent: root.size,
    }];
    while let Some(item) = stack.pop() {
        match item {
            Item::Dir {
                path,
                depth,
                size,
                count,
                parent,
            } => {
                rows.push(Row {
                    path,
                    depth,
                    share: share(size, parent),
                    kind: Kind::Folder { size, count },
                });
                if !expanded.contains(path) {
                    continue;
                }
                let Some(folder) = hierarchy.folders.get(path) else {
                    rows.push(Row {
                        path,
                        depth: depth + 1,
                        share: 0.0,
                        kind: Kind::Loading,
                    });
                    continue;
                };
                if path == current && !folder.direct_files.is_empty() {
                    stack.push(Item::Files {
                        path,
                        depth: depth + 1,
                    });
                }
                for child in folder.children.iter().rev() {
                    stack.push(Item::Dir {
                        path: child.path.as_str(),
                        depth: depth + 1,
                        size: child.size,
                        count: child.count,
                        parent: folder.size,
                    });
                }
            }
            Item::Files { path, depth } => {
                let Some(folder) = hierarchy.folders.get(path) else {
                    continue;
                };
                for file in folder.direct_files.iter().take(MAX_FILE_ROWS) {
                    rows.push(Row {
                        path: file.path.as_str(),
                        depth,
                        share: share(file.size, folder.size),
                        kind: Kind::File(file),
                    });
                }
                let hidden = folder.direct_files.len().saturating_sub(MAX_FILE_ROWS);
                if hidden > 0 || folder.files_truncated {
                    rows.push(Row {
                        path,
                        depth,
                        share: 0.0,
                        kind: Kind::More(hidden),
                    });
                }
            }
        }
    }
    rows
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum KeyAction {
    Focus(String, usize),
    Expand(String),
    Collapse(String),
    Navigate(String),
    Open(String),
}
