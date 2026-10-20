use super::watch_commit::drain_watch_burst;
use super::*;

struct ContinuousWrites {
    reads: usize,
    overflow: bool,
}

impl watch_mount::Watch for ContinuousWrites {
    fn read_batch(&mut self) -> Result<watch_linux::WatchBatch> {
        self.reads += 1;
        if self.overflow {
            Ok(watch_linux::WatchBatch::RescanRequired(
                "native queue overflow",
            ))
        } else {
            Ok(watch_linux::WatchBatch::Changes(vec![DeltaChange::Remove(
                "/busy".into(),
            )]))
        }
    }

    fn wait_readable(&self, _timeout: std::time::Duration) -> std::io::Result<bool> {
        std::thread::sleep(std::time::Duration::from_millis(1));
        Ok(true)
    }
}

#[test]
fn continuous_writes_publish_without_waiting_for_quiet() {
    let mut watcher = ContinuousWrites {
        reads: 0,
        overflow: false,
    };
    let mut pending = BTreeMap::new();
    let started = std::time::Instant::now();
    drain_watch_burst(&mut watcher, &mut pending, &AtomicBool::new(false)).unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert!(watcher.reads > 0);
    assert_eq!(pending.len(), 1);
}

#[test]
fn queue_overflow_is_reported_instead_of_serving_incomplete_updates() {
    let mut watcher = ContinuousWrites {
        reads: 0,
        overflow: true,
    };
    let error =
        drain_watch_burst(&mut watcher, &mut BTreeMap::new(), &AtomicBool::new(false)).unwrap_err();
    assert!(error.to_string().contains("native queue overflow"));
}

struct RecoverableGap {
    batches: std::collections::VecDeque<watch_linux::WatchBatch>,
}

impl watch_mount::Watch for RecoverableGap {
    fn read_batch(&mut self) -> Result<watch_linux::WatchBatch> {
        Ok(self.batches.pop_front().expect("readable batch"))
    }

    fn wait_readable(&self, _: std::time::Duration) -> std::io::Result<bool> {
        Ok(!self.batches.is_empty())
    }
}

#[test]
fn recoverable_gap_preserves_pending_and_subsequent_events() {
    let mut pending = BTreeMap::from([("/before".into(), DeltaChange::Remove("/before".into()))]);
    let requested = AtomicBool::new(false);
    let mut watcher = RecoverableGap {
        batches: [
            watch_linux::WatchBatch::ReconcileRequired("Btrfs queue overflow"),
            watch_linux::WatchBatch::Changes(vec![DeltaChange::Remove("/after".into())]),
        ]
        .into(),
    };
    drain_watch_burst(&mut watcher, &mut pending, &requested).unwrap();
    assert!(requested.load(Ordering::Acquire));
    assert!(pending.contains_key("/before"));
    assert!(pending.contains_key("/after"));
}
