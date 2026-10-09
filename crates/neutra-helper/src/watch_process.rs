use crate::watch_linux::WatchBatch;
use crate::watch_mount::{MountWatcher, Watch};
use anyhow::{bail, Context, Result};
use neutra_core::proto::{read_frame, write_frame};
use neutra_core::DeltaChange;
use std::io::{self, BufReader};
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::Duration;

type Batch = std::result::Result<Vec<DeltaChange>, String>;

// Closing any fd for an inode releases this process's POSIX locks on it.
// Mount fanotify delivers fds even for excluded SQLite files, so it must not
// run in the process that holds database and shared-memory locks.
pub(crate) struct ProcessWatcher {
    child: Child,
    output: BufReader<ChildStdout>,
}

impl Drop for ProcessWatcher {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ProcessWatcher {
    pub(crate) fn open(index: &Path, mount: &Path, source: u32) -> Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("--watch-events")
            .arg(index)
            .arg(mount)
            .arg(source.to_string())
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .context("start isolated mount event reader")?;
        let output = BufReader::new(child.stdout.take().context("mount reader has no output")?);
        let mut watcher = Self { child, output };
        if !watcher.wait_readable(Duration::from_secs(10))? {
            bail!("mount event reader did not initialize within ten seconds");
        }
        match watcher.read_batch()? {
            WatchBatch::Changes(changes) if changes.is_empty() => Ok(watcher),
            WatchBatch::RescanRequired(reason) => bail!(reason),
            _ => bail!("unexpected mount event reader greeting"),
        }
    }
}

impl Watch for ProcessWatcher {
    fn read_batch(&mut self) -> Result<WatchBatch> {
        match read_frame::<_, Batch>(&mut self.output)? {
            Some(Ok(changes)) => Ok(WatchBatch::Changes(changes)),
            Some(Err(error)) => bail!("mount event reader: {error}"),
            None => bail!("mount event reader exited: {}", self.child.wait()?),
        }
    }

    fn wait_readable(&self, timeout: Duration) -> io::Result<bool> {
        // A preceding framed read can leave the next frame in BufReader.
        if !self.output.buffer().is_empty() {
            return Ok(true);
        }
        let mut descriptor = libc::pollfd {
            fd: self.output.get_ref().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        loop {
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout.as_millis() as i32) };
            if ready >= 0 {
                return Ok(ready > 0);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
}

pub(crate) fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(2).collect();
    let [index, mount, source, parent] = args.as_slice() else {
        bail!("internal usage: --watch-events INDEX MOUNT SOURCE PARENT");
    };
    let parent = parent
        .parse::<libc::pid_t>()
        .context("invalid parent PID")?;
    // Also exit when an idle reader's parent dies without closing our pipe.
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
        return Err(io::Error::last_os_error().into());
    }
    if unsafe { libc::getppid() } != parent {
        bail!("mount event reader parent exited during startup");
    }
    let mount = crate::scan::find_local_mount(mount)?;
    let mut watcher = MountWatcher::open(
        mount,
        source.parse().context("invalid source ID")?,
        crate::scan::watch_exclusions(Path::new(index)),
    )?;
    let mut output = std::io::stdout().lock();
    write_frame(&mut output, &Batch::Ok(Vec::new()))?;
    loop {
        let batch = match watcher.read_batch() {
            Ok(WatchBatch::Changes(changes)) if changes.is_empty() => continue,
            Ok(WatchBatch::Changes(changes)) => Ok(changes),
            Ok(WatchBatch::RescanRequired(reason)) => Err(reason.to_string()),
            Err(error) => Err(format!("{error:#}")),
        };
        let failed = batch.is_err();
        write_frame(&mut output, &batch)?;
        if failed {
            return Ok(());
        }
    }
}
