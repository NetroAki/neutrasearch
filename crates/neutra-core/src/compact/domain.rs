use super::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub(crate) fn collect_trigrams(text: &str, out: &mut HashSet<u32>) {
    let folded = text.to_lowercase();
    for w in folded.as_bytes().windows(3) {
        out.insert((w[0] as u32) << 16 | (w[1] as u32) << 8 | w[2] as u32);
    }
}

pub(super) fn intersect(a: &[u32], b: &[u32]) -> Vec<u32> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}
pub(super) fn retain_best<F>(ranked: &mut Vec<(u32, FileRecord)>, limit: usize, compare: &F)
where
    F: Fn(&(u32, FileRecord), &(u32, FileRecord)) -> std::cmp::Ordering,
{
    if ranked.len() > limit {
        ranked.select_nth_unstable_by(limit, compare);
        ranked.truncate(limit);
    }
}

/// Byte fold shared by index ordering and prefix scans so both agree on
/// every platform.
pub(crate) fn fold_byte(byte: u8) -> u8 {
    #[cfg(target_os = "windows")]
    let byte = if byte == b'\\' { b'/' } else { byte };
    #[cfg(target_os = "windows")]
    {
        byte.to_ascii_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        byte
    }
}

pub(super) fn folded_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .bytes()
            .zip(right.bytes())
            .all(|(left, right)| fold_byte(left) == fold_byte(right))
}

pub(crate) fn compare_index_paths(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    for (left, right) in left.iter().zip(right) {
        match fold_byte(*left).cmp(&fold_byte(*right)) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    left.len().cmp(&right.len())
}

pub(super) fn equivalent_index_path(left: &str, right: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        left.replace('\\', "/").to_lowercase() == right.replace('\\', "/").to_lowercase()
    }
    #[cfg(not(target_os = "windows"))]
    {
        left == right
    }
}

pub(crate) fn suffix_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}
