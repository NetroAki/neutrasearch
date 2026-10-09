use neutra_core::{BrowserIndex, CompactIndex, FileKind, FileRecord, FsKind, SpillAccumulator};
use std::{io, path::PathBuf};

fn main() -> io::Result<()> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("isolated fixture directory required");
    if root.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "fixture must be new",
        ));
    }
    let mut records = Vec::new();
    for (folder, extension) in [("Music", "wav"), ("Photos", "png"), ("Documents", "txt")] {
        let directory = root.join(folder);
        std::fs::create_dir_all(&directory)?;
        records.push(record(&directory)?);
        for number in 0..450 {
            let path = directory.join(format!("QA {folder} {number:03}.{extension}"));
            std::fs::write(&path, vec![b'x'; (number + 1) * 32])?;
            records.push(record(&path)?);
        }
    }
    records.push(record(&root)?);
    let index = root.join("fixture.nsx");
    let mut spill = SpillAccumulator::begin(&index)?;
    spill.push_batch(records)?;
    let built = CompactIndex::rebuild_streamed(spill.finish()?, &index)?;
    BrowserIndex::ensure(&index, built.generation)?;
    println!("{}", index.display());
    Ok(())
}

fn record(path: &std::path::Path) -> io::Result<FileRecord> {
    let meta = std::fs::metadata(path)?;
    #[cfg(unix)]
    let allocated = {
        use std::os::unix::fs::MetadataExt;
        meta.blocks() * 512
    };
    #[cfg(not(unix))]
    let allocated = meta.len();
    Ok(FileRecord {
        path: path.to_string_lossy().into_owned().into_boxed_str(),
        size: meta.len(),
        disk: FileRecord::allocated_bytes(allocated),
        mtime: meta
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_secs() as i64,
        mode: 0,
        kind: if meta.is_dir() {
            FileKind::Dir
        } else {
            FileKind::File
        },
        fs: FsKind::Btrfs,
        native_id: 0,
        native_parent: 0,
        source: 0,
    })
}
