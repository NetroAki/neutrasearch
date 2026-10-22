//! Namespace validation and cached directory path construction.
//!
//! Intentional omissions (never failures): `"."`/`".."` (filtered by the
//! record parser), alternate data streams (never name records), DOS-namespace
//! (0x02) aliases (never primary names), and names visible only through an
//! unresolved `$ATTRIBUTE_LIST` (counted as skipped, not invented). Reserved
//! MFT records 0-15 are not excluded by id: test volumes reuse those ids and
//! real metadata with a resolvable Win32 name is legitimate native metadata.
use crate::records::{append_component, Alias, Entry};
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Failure {
    Missing(u64),
    Sequence { id: u64, expected: u16, actual: u16 },
    Cycle,
    NonDirectory(u64),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Missing(id) => write!(f, "parent record {id} is missing"),
            Self::Sequence {
                id,
                expected,
                actual,
            } => write!(
                f,
                "parent record {id} sequence mismatch (expected {expected}, got {actual})"
            ),
            Self::Cycle => f.write_str("parent chain contains a cycle"),
            Self::NonDirectory(id) => write!(f, "parent record {id} is not a directory"),
        }
    }
}

pub(crate) fn ensure_dir_path(
    id: u64,
    expected_sequence: u16,
    root_sequence: u16,
    entries: &HashMap<u64, Entry>,
    cache: &mut HashMap<u64, String>,
    seen: &mut HashSet<u64>,
) -> std::result::Result<(), Failure> {
    if id == 5 {
        return sequence_matches(id, expected_sequence, root_sequence);
    }
    if let Some(entry) = entries.get(&id) {
        if expected_sequence != 0 && entry.sequence != expected_sequence {
            return Err(Failure::Sequence {
                id,
                expected: expected_sequence,
                actual: entry.sequence,
            });
        }
        if cache.contains_key(&id) {
            if !entry.dir {
                return Err(Failure::NonDirectory(id));
            }
            return Ok(());
        }
    }

    let mut current = id;
    let mut expected = expected_sequence;
    let mut chain = Vec::new();
    seen.clear();
    loop {
        if current == 5 {
            sequence_matches(5, expected, root_sequence)?;
            break;
        }
        if cache.contains_key(&current) {
            if expected != 0 {
                let entry = entries.get(&current).ok_or(Failure::Missing(current))?;
                if entry.sequence != expected {
                    return Err(Failure::Sequence {
                        id: current,
                        expected,
                        actual: entry.sequence,
                    });
                }
            }
            break;
        }
        if !seen.insert(current) {
            return Err(Failure::Cycle);
        }
        let entry = entries.get(&current).ok_or(Failure::Missing(current))?;
        if expected != 0 && entry.sequence != expected {
            return Err(Failure::Sequence {
                id: current,
                expected,
                actual: entry.sequence,
            });
        }
        if !entry.dir {
            return Err(Failure::NonDirectory(current));
        }
        chain.push(current);
        current = entry.parent;
        expected = entry.parent_sequence;
    }

    let mut path = cache.get(&current).cloned().unwrap_or_default();
    for record in chain.into_iter().rev() {
        path = append_component(&path, &entries[&record].name);
        cache.insert(record, path.clone());
    }
    Ok(())
}

fn sequence_matches(id: u64, expected: u16, actual: u16) -> std::result::Result<(), Failure> {
    if expected == 0 || expected == actual {
        Ok(())
    } else {
        Err(Failure::Sequence {
            id,
            expected,
            actual,
        })
    }
}

pub(crate) fn validate(
    entries: &HashMap<u64, Entry>,
    aliases: &HashMap<u64, Vec<Alias>>,
    root_allocated: bool,
    root_dir: bool,
    root_sequence: u16,
    dir_paths: &mut HashMap<u64, String>,
) -> Result<()> {
    if !root_allocated {
        bail!("NTFS root MFT record 5 is missing or unallocated");
    }
    if !root_dir {
        bail!("NTFS root MFT record 5 is not a directory");
    }
    dir_paths.insert(5, String::new());
    let mut seen = HashSet::new();
    for (&id, entry) in entries {
        if id == 5 {
            continue;
        }
        ensure_dir_path(
            entry.parent,
            entry.parent_sequence,
            root_sequence,
            entries,
            dir_paths,
            &mut seen,
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "MFT record {id} (\"{}\") has no path to the volume root: {e}",
                entry.name
            )
        })?;
        if entry.dir {
            ensure_dir_path(
                id,
                entry.sequence,
                root_sequence,
                entries,
                dir_paths,
                &mut seen,
            )
            .map_err(|e| {
                anyhow::anyhow!("MFT directory {id} has no path to the volume root: {e}")
            })?;
        }
    }
    for (&id, names) in aliases {
        let Some(entry) = entries.get(&id) else {
            continue;
        };
        if entry.dir {
            continue;
        }
        for alias in names {
            ensure_dir_path(
                alias.parent,
                alias.parent_sequence,
                root_sequence,
                entries,
                dir_paths,
                &mut seen,
            )
            .map_err(|e| {
                anyhow::anyhow!(
                    "MFT record {id} alias (\"{}\") has no path to the volume root: {e}",
                    alias.name
                )
            })?;
        }
    }
    Ok(())
}
