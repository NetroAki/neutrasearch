use super::*;

pub(super) struct Session {
    pub(super) out: ProtocolOutput,
    pub(super) index: Arc<RwLock<Index>>,
    pub(super) durable: Option<Arc<RwLock<DurableStore>>>,
    pub(super) stale: Arc<AtomicBool>,
    pub(super) scan_threads: Vec<std::thread::JoinHandle<()>>,
}

impl Session {
    pub(super) fn open(
        out: ProtocolOutput,
        serve_index: Option<std::path::PathBuf>,
        watch_mount: Option<(std::path::PathBuf, u32)>,
    ) -> Result<Self> {
        if let Some(path) = serve_index.as_ref() {
            neutra_core::CompactIndex::recover_rebuild(path)
                .context("recover interrupted full rebuild")?;
        }
        // Scans populate one resident index; searches never trigger a rescan.
        let index = Arc::new(RwLock::new(Index::default()));
        #[cfg(target_os = "linux")]
        let watches = watch_mount
            .as_ref()
            .map(|(mount, source)| {
                crate::watch_session::WatchSession::prepare(
                    serve_index
                        .as_ref()
                        .context("watch mode requires an index")?,
                    mount,
                    *source,
                )
            })
            .transpose()?;
        let durable = serve_index
            .as_ref()
            .map(|path| DurableStore::open(path).map(|store| Arc::new(RwLock::new(store))))
            .transpose()?;
        let stale = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        if let Some(watches) = watches {
            let store = Arc::clone(durable.as_ref().expect("watch mode has a durable store"));
            watches.start(store, Arc::clone(&stale))?;
            anyhow::ensure!(
                !stale.load(Ordering::Acquire),
                "native watcher failed during startup"
            );
            send(&out, &HelperMsg::WatchReady)?;
        }
        #[cfg(target_os = "linux")]
        if let Some(store) = durable.as_ref() {
            DurableStore::start_maintenance(store, Arc::clone(&stale))?;
        }
        #[cfg(not(target_os = "linux"))]
        if watch_mount.is_some() {
            anyhow::bail!(
                "native watch mode is not implemented on {}",
                std::env::consts::OS
            );
        }

        Ok(Self {
            out,
            index,
            durable,
            stale,
            scan_threads: Vec::new(),
        })
    }
}
