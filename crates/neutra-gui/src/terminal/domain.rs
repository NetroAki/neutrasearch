use neutra_core::proto::HelperMsg;
use neutra_core::{CompactIndex, SpillAccumulator};
use std::path::{Path, PathBuf};
/// Precompute listings so the GUI's first screen is instant. Failures defer work to the GUI.
pub(super) fn build_ranked_lists(path: &Path) {
    let built = CompactIndex::open_fast(path).and_then(|index| {
        neutra_core::BrowserIndex::ensure(path, index.generation())?;
        neutra_core::TreeSummary::ensure(path, index.generation())
    });
    if let Err(message) = built {
        eprintln!("neutrasearch: sorted lists not built: {message}");
    }
}

pub(super) fn build_machine_index_on_large_stack(
    output: PathBuf,
    allow_zfs_enumerate: bool,
) -> Result<(), String> {
    let worker = std::thread::Builder::new()
        .name("neutrasearch-index".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || build_machine_index(&output, allow_zfs_enumerate))
        .map_err(|error| format!("cannot start index builder: {error}"))?;
    worker
        .join()
        .map_err(|_| "the index builder overflowed its stack".to_string())?
}

pub(super) fn parse_index(mut args: Vec<std::ffi::OsString>) -> Result<Option<PathBuf>, String> {
    let mut output = None;
    while !args.is_empty() {
        let option = args.remove(0);
        if option == "--output" || option == "-o" {
            if args.is_empty() {
                return Err("--output requires a path".into());
            }
            if output.is_some() {
                return Err("--output may be specified only once".into());
            }
            output = Some(PathBuf::from(args.remove(0)));
        } else {
            return Err(format!(
                "unknown index option {}; indexing always covers the full machine",
                option.to_string_lossy()
            ));
        }
    }
    Ok(output)
}

pub(super) fn index_override(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let mut index = None;
    let mut position = 0;
    while position < args.len() {
        if args[position] == "--index" {
            let value = args
                .get(position + 1)
                .ok_or_else(|| "--index requires a path".to_string())?;
            if index.replace(PathBuf::from(value)).is_some() {
                return Err("--index may be specified only once".into());
            }
            position += 2;
        } else {
            position += 1;
        }
    }
    Ok(index)
}

pub(super) fn ensure_index_without_build(path: &Path) -> Result<(), String> {
    if let Ok((index, _delta)) = CompactIndex::open_with_delta_snapshot_fast(path) {
        drop(index);
        neutra_core::paths::remember_index_path(path)
            .map_err(|error| format!("cannot remember {}: {error}", path.display()))?;
        return Ok(());
    }
    Err(format!(
        "no usable index at {}; build one first with `neutrasearch index`",
        path.display()
    ))
}

pub(super) fn ensure_index(path: &Path) -> Result<(), String> {
    match CompactIndex::open_with_delta_snapshot_fast(path) {
        Ok((index, _delta)) => {
            drop(index);
            neutra_core::paths::remember_index_path(path)
                .map_err(|error| format!("cannot remember {}: {error}", path.display()))?;
            return Ok(());
        }
        Err(error) if path.exists() => {
            eprintln!(
                "neutrasearch: index at {} is not usable ({error}); indexing the full machine first",
                path.display()
            );
        }
        Err(_) => {
            eprintln!(
                "neutrasearch: no usable index exists at {}; indexing the full machine first",
                path.display()
            );
        }
    }
    build_machine_index(path, false)
}

pub(super) fn build_machine_index(output: &Path, allow_zfs_enumerate: bool) -> Result<(), String> {
    let roots = super::default_system_roots();
    let mounts = super::selected_scan_mounts(&roots);
    if mounts.is_empty() {
        return Err("no supported local native filesystems were discovered".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    super::spawn_local_helper(
        crate::app::EventSender::new(tx, egui::Context::default()),
        cfg!(target_os = "linux"),
        mounts,
        roots.clone(),
        allow_zfs_enumerate,
    );
    let mut spill = SpillAccumulator::begin(output)
        .map_err(|error| format!("cannot spill scan batches: {error}"))?;
    let mut cursors = Vec::new();
    let mut lane_failed = false;
    let completion = loop {
        match rx.recv() {
            Ok(super::Event::Message(HelperMsg::ScanBegin { mount })) => {
                eprintln!("neutrasearch: indexing {}", mount.mountpoint.display());
            }
            Ok(super::Event::Message(HelperMsg::ScanCursor { cursor })) => cursors.push(cursor),
            Ok(super::Event::Message(HelperMsg::Records(records))) => {
                spill
                    .push_batch(
                        records
                            .into_iter()
                            .filter(|record| super::record_in_roots(record.path.as_ref(), &roots))
                            .collect(),
                    )
                    .map_err(|error| format!("cannot spill scan batches: {error}"))?;
            }
            Ok(super::Event::Message(HelperMsg::ScanDone { mount, stats })) => {
                eprintln!(
                    "neutrasearch: indexed {} records from {} in {} ms",
                    stats.records,
                    mount.mountpoint.display(),
                    stats.wall_ms
                );
            }
            Ok(super::Event::Message(HelperMsg::ScanError { mount, error })) => {
                lane_failed = true;
                eprintln!(
                    "neutrasearch: skipped {}: {error}",
                    mount.mountpoint.display()
                );
            }
            Ok(super::Event::Message(HelperMsg::ScanComplete { mounts, errors })) => {
                break Ok((mounts, errors));
            }
            Ok(super::Event::Message(HelperMsg::Error(error))) => break Err(error),
            Ok(super::Event::Fatal(error)) => break Err(error),
            Ok(_) => {}
            Err(_) => break Err("native scanner stopped before completing".into()),
        }
    };
    let (completed_mounts, errors) = completion?;
    if let Some(message) =
        scan_completion_error(completed_mounts, errors.max(u32::from(lane_failed)))
    {
        return Err(message);
    }
    if spill.is_empty() {
        return Err("native scanners returned no files; the previous index was kept".into());
    }
    let built = CompactIndex::rebuild_streamed_with_cursors(
        spill
            .finish()
            .map_err(|error| format!("cannot publish {}: {error}", output.display()))?,
        output,
        &cursors,
    )
    .map_err(|error| format!("cannot publish {}: {error}", output.display()))?;
    let remembered = neutra_core::paths::remember_index_path(output)
        .map_err(|error| format!("cannot remember {}: {error}", output.display()))?;
    println!(
        "indexed={} bytes={} build_ms={} output={}",
        built.records,
        built.bytes,
        built.wall_ms,
        remembered.display()
    );
    Ok(())
}

pub(super) fn scan_completion_error(completed_mounts: u32, errors: u32) -> Option<String> {
    (completed_mounts == 0 || errors > 0)
        .then(|| format!("native scan failed ({errors} error(s)); previous index was kept"))
}
