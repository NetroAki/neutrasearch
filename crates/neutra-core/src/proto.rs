//! Framed wire protocol between clients (GUI / MCP / Pi plugin) and
//! neutrasearch-helper (local privileged process or remote server helper).
//!
//! Framing: 4-byte little-endian length + bincode payload. Identical over a
//! child's stdout pipe or an authenticated SSH child process. This protocol is
//! not a network authentication layer and must not be exposed as a TCP service.

use crate::delta::DeltaChange;
use crate::mounts::MountInfo;
use crate::query::Query;
use crate::types::{FileRecord, ScanStats};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

/// Protocol version; helper and client refuse to talk across major versions.
pub const PROTO_VERSION: u32 = 7;

/// Bump this whenever the helper binary changes in a way that affects
/// auto-provisioning decisions (client pushes a fresh copy when the remote
/// reports an older build).
pub const HELPER_BUILD: u32 = 9;
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub enum ClientMsg {
    /// Identify + negotiate. Must be the first message.
    Hello { proto: u32 },
    /// Scan the given mounts and stream only records inside the approved roots.
    /// Empty mounts or roots scan nothing; all-volume scans must enumerate both.
    Scan {
        mounts: Vec<MountInfo>,
        roots: Vec<std::path::PathBuf>,
    },
    /// Scan, filter to approved roots, and retain records for later searches.
    /// Empty mounts or roots scan nothing.
    ScanResident {
        mounts: Vec<MountInfo>,
        roots: Vec<std::path::PathBuf>,
    },
    /// Query the helper's explicitly resident index.
    Search { query: Query },
    /// Persist a batch into the generation-bound delta WAL before publishing it.
    ApplyDelta { changes: Vec<DeltaChange> },
    /// Ask the helper to stop scanning and exit cleanly.
    Shutdown,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HelperMsg {
    Hello {
        proto: u32,
        build: u32,
        os: String,
        arch: String,
    },
    ScanBegin {
        mount: MountInfo,
    },
    /// Batched to keep per-frame overhead tiny.
    Records(Vec<FileRecord>),
    ScanDone {
        mount: MountInfo,
        stats: ScanStats,
    },
    ScanError {
        mount: MountInfo,
        error: String,
    },
    /// Terminates one Scan/ScanResident request, including the zero-mount case.
    /// Fail-closed clients publish only when `errors == 0`. Interactive clients
    /// may publish records from reachable lanes when `errors < mounts`, but must
    /// surface the degraded result and replace—not retain—failed-source records.
    ScanComplete {
        mounts: u32,
        errors: u32,
    },
    /// Response to Search: matched records (already sorted/limited).
    SearchResult {
        hits: Vec<FileRecord>,
        wall_us: u64,
    },
    DeltaApplied {
        changes: u32,
        wal_bytes: u64,
        needs_compaction: bool,
    },
    Error(String),
}

pub fn write_frame<W: Write>(w: &mut W, msg: &impl Serialize) -> bincode::Result<()> {
    let payload = bincode::serialize(msg)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(Box::new(bincode::ErrorKind::Custom(format!(
            "frame length {} exceeds cap",
            payload.len()
        ))));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| Box::new(bincode::ErrorKind::Custom("frame too large".into())))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&payload)?;
    w.flush().map_err(bincode::Error::from)?;
    Ok(())
}

pub fn read_frame<R, T>(r: &mut R) -> bincode::Result<Option<T>>
where
    R: Read,
    T: for<'de> Deserialize<'de>,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(Box::new(bincode::ErrorKind::Io(e))),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    // Hard cap: a malicious or corrupted peer must not force an unbounded allocation.
    if len > MAX_FRAME_BYTES {
        return Err(Box::new(bincode::ErrorKind::Custom(format!(
            "frame length {len} exceeds cap"
        ))));
    }
    let mut payload = vec![0u8; len];
    r.read_exact(&mut payload)?;
    bincode::deserialize(&payload).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::Query;

    #[test]
    fn oversized_frame_is_rejected_before_payload_allocation() {
        let length = u32::try_from(MAX_FRAME_BYTES + 1).unwrap().to_le_bytes();
        let error = read_frame::<_, ClientMsg>(&mut &length[..]).unwrap_err();
        assert!(error.to_string().contains("exceeds cap"));
    }

    #[test]
    fn scan_completion_roundtrip_preserves_atomic_publication_counts() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &HelperMsg::ScanComplete {
                mounts: 3,
                errors: 1,
            },
        )
        .unwrap();
        let message: HelperMsg = read_frame(&mut bytes.as_slice()).unwrap().unwrap();
        assert!(matches!(
            message,
            HelperMsg::ScanComplete {
                mounts: 3,
                errors: 1
            }
        ));
    }

    #[test]
    fn frame_roundtrip() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &ClientMsg::Hello {
                proto: PROTO_VERSION,
            },
        )
        .unwrap();
        write_frame(
            &mut buf,
            &ClientMsg::Search {
                query: Query::parse("main ext:rs"),
            },
        )
        .unwrap();
        let mut slice = &buf[..];
        let m1: ClientMsg = read_frame(&mut slice).unwrap().unwrap();
        assert!(matches!(
            m1,
            ClientMsg::Hello {
                proto: PROTO_VERSION
            }
        ));
        let m2: ClientMsg = read_frame(&mut slice).unwrap().unwrap();
        match m2 {
            ClientMsg::Search { query } => assert_eq!(query.terms, vec!["main"]),
            _ => panic!("wrong message"),
        }
        let eof: Option<ClientMsg> = read_frame(&mut slice).unwrap();
        assert!(eof.is_none());
    }
}
