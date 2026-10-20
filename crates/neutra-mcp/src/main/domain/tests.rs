use super::*;
use crate::{
    policy::{path_is_allowed, portable_path_is_under, portable_path_text},
    store::Store,
};
use neutra_core::{FileKind, FileRecord, FsKind, Index};
use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

fn empty_store() -> Store {
    Store::Legacy {
        path: PathBuf::from("test-index.bin"),
        index: Index::new(),
    }
}

fn build_test_base(records: &[FileRecord], path: &Path) {
    let mut spill = neutra_core::SpillAccumulator::begin(path).unwrap();
    spill.push_batch(records.to_vec()).unwrap();
    neutra_core::CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
}

#[path = "tests/directory.rs"]
mod directory;
#[path = "tests/policy.rs"]
mod policy;
#[path = "tests/protocol.rs"]
mod protocol;
