use super::super::Hierarchy;
use super::super::*;

#[derive(Clone)]
pub(in crate::ui::treemap) struct MapBlock {
    pub(in crate::ui::treemap) path: String,
    pub(in crate::ui::treemap) name: String,
    pub(in crate::ui::treemap) bytes: u64,
    pub(in crate::ui::treemap) count: u64,
    pub(in crate::ui::treemap) folder: bool,
    pub(in crate::ui::treemap) extension: String,
}

pub(in crate::ui::treemap) fn map_blocks(hierarchy: &Hierarchy, current: &str) -> Vec<MapBlock> {
    enum Child {
        Dir(usize),
        File(usize),
    }
    let Some(folder) = hierarchy.folders.get(current) else {
        return Vec::new();
    };
    // Subdirectory totals ride along in the parent listing, so tiles never
    // force-fetch child directories.
    let mut candidates: Vec<(u64, Child)> = Vec::new();
    for (index, child) in folder.children.iter().enumerate() {
        candidates.push((child.size.max(1), Child::Dir(index)));
    }
    for (index, file) in folder.direct_files.iter().enumerate() {
        candidates.push((file.size.max(1), Child::File(index)));
    }
    if candidates.len() > 256 {
        candidates.select_nth_unstable_by_key(255, |(bytes, _)| std::cmp::Reverse(*bytes));
        candidates.truncate(256);
    }
    candidates.sort_unstable_by_key(|candidate| std::cmp::Reverse(candidate.0));
    candidates
        .into_iter()
        .map(|(_, child)| match child {
            Child::Dir(index) => {
                let child = &folder.children[index];
                MapBlock {
                    path: child.path.clone(),
                    name: path_name(&child.path),
                    bytes: child.size,
                    count: child.count,
                    folder: true,
                    extension: String::new(),
                }
            }
            Child::File(index) => {
                let file = &folder.direct_files[index];
                let name = path_name(&file.path);
                MapBlock {
                    path: file.path.clone(),
                    name: name.clone(),
                    bytes: file.size,
                    count: 1,
                    folder: false,
                    extension: name
                        .rsplit_once('.')
                        .map_or(String::new(), |(_, extension)| {
                            extension.to_ascii_lowercase()
                        }),
                }
            }
        })
        .collect()
}

pub(in crate::ui::treemap) fn layout_map<'a>(
    items: &'a [MapBlock],
    rect: Rect,
    out: &mut Vec<(&'a MapBlock, Rect)>,
) {
    if items.is_empty() || rect.width() < 2.0 || rect.height() < 2.0 {
        return;
    }
    if items.len() == 1 {
        out.push((&items[0], rect));
        return;
    }
    let total = items
        .iter()
        .map(|item| item.bytes.max(1))
        .sum::<u64>()
        .max(1);
    let mut left = 0u64;
    let mut split = 1usize;
    for (index, item) in items.iter().enumerate().take(items.len() - 1) {
        left = left.saturating_add(item.bytes.max(1));
        split = index + 1;
        if left >= total / 2 {
            break;
        }
    }
    let ratio = (left as f32 / total as f32).clamp(0.08, 0.92);
    let (first, second) = if rect.width() >= rect.height() {
        let x = rect.left() + rect.width() * ratio;
        (
            Rect::from_min_max(rect.min, egui::pos2(x, rect.bottom())),
            Rect::from_min_max(egui::pos2(x, rect.top()), rect.max),
        )
    } else {
        let y = rect.top() + rect.height() * ratio;
        (
            Rect::from_min_max(rect.min, egui::pos2(rect.right(), y)),
            Rect::from_min_max(egui::pos2(rect.left(), y), rect.max),
        )
    };
    layout_map(&items[..split], first, out);
    layout_map(&items[split..], second, out);
}
