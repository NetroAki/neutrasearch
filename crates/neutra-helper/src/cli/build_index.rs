use super::*;
use crate::scan::{exclusion_prefixes, path_has_component_prefix};

pub(super) fn run() -> Result<()> {
    let positional: Vec<_> = std::env::args().skip(2).collect();
    let [target, output] = positional.as_slice() else {
        anyhow::bail!("use: neutrasearch index MOUNT --output INDEX.nsx");
    };
    let output = std::path::PathBuf::from(output);
    let mount = find_local_mount(target)?;
    let mountpoint = mount.mountpoint.clone();
    let exclusions = exclusion_prefixes(&mountpoint);
    // Stream scan batches straight into the spill: one huge mount
    // materialized here peaked past 25 GiB.
    let mut spill = neutra_core::SpillAccumulator::begin(&output)?;
    let mut spill_error: Option<std::io::Error> = None;
    let scan = dispatch_lane(&mount, &mut |record| {
        if spill_error.is_some() {
            return;
        }
        if !exclusions
            .iter()
            .any(|prefix| path_has_component_prefix(record.path.as_ref(), prefix))
        {
            if let Err(error) = spill.push_batch(vec![record]) {
                spill_error = Some(error);
            }
        }
    })?;
    if let Some(error) = spill_error {
        anyhow::bail!("spill scan batches: {error}");
    }
    let built = CompactIndex::rebuild_streamed(spill.finish()?, &output)?;
    println!("fs={} mount={} records={} scan_ms={} index_bytes={} blocks={} trigrams={} build_ms={} output={}",mount.fs.label(),mount.mountpoint.display(),scan.records,scan.wall_ms,built.bytes,built.blocks,built.trigrams,built.wall_ms,output.display());
    Ok(())
}
