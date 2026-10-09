use crate::protocol::start_native_watch;
use crate::store::{write_watch_status, DurableStore};
use crate::watch_linux::FanotifyWatcher;
use crate::watch_mount::{needs_mount_mark, Watch};
use crate::watch_process::ProcessWatcher;
use crate::watch_sweep::PreparedSweep;
use anyhow::{Context, Result};
use neutra_core::{FsKind, MountInfo};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, RwLock};

pub(crate) struct WatchSession {
    readers: Vec<Box<dyn Watch>>,
    sweep: PreparedSweep,
    limitations: Vec<String>,
    source: u32,
}

impl WatchSession {
    // Attach before opening/recovering the store so startup writes queue safely.
    pub(crate) fn prepare(base: &Path, mountpoint: &Path, source: u32) -> Result<Self> {
        let mounts = neutra_core::mounts::system_mounts()?;
        let mounts: Vec<MountInfo> = if mountpoint == Path::new("/") {
            mounts
                .into_iter()
                .filter(|mount| mount.fs.is_indexable_local())
                .collect()
        } else {
            vec![mounts
                .into_iter()
                .find(|mount| mount.mountpoint == mountpoint)
                .with_context(|| format!("no supported mount at {}", mountpoint.display()))?]
        };
        anyhow::ensure!(
            !mounts.is_empty(),
            "no supported local filesystem is mounted; native watches cannot start"
        );
        let excluded = crate::scan::watch_exclusions(base);
        let sweep = PreparedSweep::prepare(&mounts, base)?;
        let mut readers: Vec<Box<dyn Watch>> = Vec::new();
        let mut limitations = Vec::new();
        for mount in mounts {
            tracing::info!(mount = %mount.mountpoint.display(), fs = %mount.fs.label(), "attaching native watch");
            match FanotifyWatcher::open(mount.clone(), source, excluded.clone()) {
                Ok(reader) => readers.push(Box::new(reader)),
                Err(error) if needs_mount_mark(&error, &mount.fs) => {
                    if mount.fs != FsKind::Btrfs {
                        // ponytail: FUSE supplies saves only; add a native NTFS journal for renames/deletes.
                        limitations.push(format!("{}: saved files update live; rename/delete journal updates are unavailable", mount.mountpoint.display()));
                    }
                    readers.push(Box::new(ProcessWatcher::open(
                        base,
                        &mount.mountpoint,
                        source,
                    )?));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "native watcher failed at {} ({})",
                            mount.mountpoint.display(),
                            mount.fs.label()
                        )
                    })
                }
            }
        }
        Ok(Self {
            readers,
            sweep,
            limitations,
            source,
        })
    }

    pub(crate) fn start(
        self,
        store: Arc<RwLock<DurableStore>>,
        stale: Arc<AtomicBool>,
    ) -> Result<()> {
        let path = store.read().unwrap().path.clone();
        write_watch_status(&path, &self.limitations.join("\n"))?;
        for reader in self.readers {
            start_native_watch(reader, Arc::clone(&store), Arc::clone(&stale));
        }
        self.sweep.start(store, stale, self.source);
        Ok(())
    }
}
