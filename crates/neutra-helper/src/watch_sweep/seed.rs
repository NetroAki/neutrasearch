use super::*;
use state::{apply_seed, restore};

#[derive(Clone, Copy, Debug)]
pub(super) struct SeedBinding {
    pub generation: Option<u64>,
    pub identity: u32,
}

/// Apply the generation-bound ingest checkpoint. Missing seed is tolerated
/// only for a complete legacy sweep checkpoint covering every mounted tree.
pub(super) fn prepare_seed(
    watched: &mut [Watched],
    base: &Path,
    state: &Path,
) -> Result<SeedBinding> {
    if watched.is_empty() {
        return Ok(SeedBinding {
            generation: None,
            identity: 0,
        });
    }
    let generation = neutra_core::CompactIndex::open_fast(base)?.generation();
    match neutra_core::native_cursor::load_seed(base, generation) {
        Ok(cursors) => {
            let identity = neutra_core::native_cursor::seed_identity(&cursors)?;
            apply_seed(watched, &cursors)?;
            let _ = restore(watched, state, Some(identity))?;
            Ok(SeedBinding {
                generation: Some(generation),
                identity,
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let complete = restore(watched, state, None)?;
            anyhow::ensure!(
                complete,
                "legacy native recovery cursor does not cover every mounted Btrfs subvolume"
            );
            let cursors = watched
                .iter()
                .map(|item| neutra_core::NativeCursor {
                    mount: item.mount.mountpoint.to_string_lossy().into_owned(),
                    tree: item.sub.tree_id(),
                    since: item.since,
                })
                .collect::<Vec<_>>();
            neutra_core::native_cursor::save_seed(base, generation, &cursors)?;
            let identity = neutra_core::native_cursor::seed_identity(&cursors)?;
            for item in watched {
                item.active = true;
            }
            Ok(SeedBinding {
                generation: Some(generation),
                identity,
            })
        }
        Err(error) => Err(error.into()),
    }
}
