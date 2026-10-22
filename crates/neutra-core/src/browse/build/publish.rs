use crate::sqlite;
use rusqlite::{
    backup::{Backup, StepResult},
    OpenFlags,
};
use std::{io, path::Path, time::Duration};

const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// Replace the contents of an existing catalog through SQLite so its live
/// connections and WAL remain associated with the same database file.
pub(super) fn replace(staged: &Path, destination: &Path) -> io::Result<()> {
    let source = sqlite::open(
        staged,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(super::super::sql_error)?;
    let target_flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW;
    if !destination.exists() {
        drop(crate::compact_build::open_private(destination)?);
    }
    let mut target = sqlite::open(destination, target_flags).map_err(super::super::sql_error)?;
    source
        .busy_timeout(LOCK_TIMEOUT)
        .map_err(super::super::sql_error)?;
    target
        .busy_timeout(LOCK_TIMEOUT)
        .map_err(super::super::sql_error)?;
    target
        .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
        .map_err(super::super::sql_error)?;
    let backup = Backup::new(&source, &mut target).map_err(super::super::sql_error)?;
    let deadline = std::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        match backup.step(128).map_err(super::super::sql_error)? {
            StepResult::Done => break,
            StepResult::More if std::time::Instant::now() < deadline => {}
            StepResult::Busy | StepResult::Locked if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            StepResult::Busy => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "SQLite catalog backup remained busy",
                ))
            }
            StepResult::Locked => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "SQLite catalog backup remained locked",
                ))
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "SQLite catalog backup timed out",
                ))
            }
        }
    }
    drop(backup);
    target
        .close()
        .map_err(|(_, error)| super::super::sql_error(error))?;
    source
        .close()
        .map_err(|(_, error)| super::super::sql_error(error))?;
    std::fs::remove_file(staged)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
