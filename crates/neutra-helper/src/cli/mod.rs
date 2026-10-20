use crate::scan::{dispatch_lane, find_local_mount};
use anyhow::{Context, Result};
use neutra_core::proto::HELPER_BUILD;
use neutra_core::CompactIndex;
mod build_index;

pub(crate) struct Options {
    pub(crate) serve_index: Option<std::path::PathBuf>,
    pub(crate) watch_mount: Option<(std::path::PathBuf, u32)>,
}

pub(crate) fn parse() -> Result<Option<Options>> {
    // `--version`/`--build` are used by auto-provisioning to decide whether a
    // remote copy is stale. Keep them dependency-free and instant.
    let arg = std::env::args().nth(1);
    let watch_mount = if arg.as_deref() == Some("--watch-index") {
        Some((
            std::path::PathBuf::from(
                std::env::args()
                    .nth(3)
                    .context("internal usage: --watch-index INDEX.nsx MOUNT [SOURCE]")?,
            ),
            std::env::args()
                .nth(4)
                .map(|source| source.parse::<u32>().context("invalid source ID"))
                .transpose()?
                .unwrap_or(0),
        ))
    } else {
        None
    };
    let serve_index = if matches!(arg.as_deref(), Some("--serve-index" | "--watch-index")) {
        Some(std::path::PathBuf::from(std::env::args().nth(2).context(
            "internal usage: neutrasearch-helper --serve-index INDEX.nsx",
        )?))
    } else {
        std::env::var_os("NEUTRASEARCH_SERVE_INDEX").map(std::path::PathBuf::from)
    };
    match arg.as_deref() {
        Some("--version") | Some("-V") => {
            println!(
                "neutrasearch-helper {} build {}",
                env!("CARGO_PKG_VERSION"),
                HELPER_BUILD
            );
            return Ok(None);
        }
        Some("--build") => {
            println!("{HELPER_BUILD}");
            return Ok(None);
        }
        Some("--serve-index") | Some("--watch-index") => {}
        Some("--zfs-probe") => {
            let probe = neutra_zfs::probe();
            println!(
                "zfs_cli={} libzpool={}",
                probe.zfs_cli_version.as_deref().unwrap_or("not installed"),
                probe.libzpool_soname.unwrap_or("not found"),
            );
            match neutra_zfs::reader_version() {
                Ok(version) => println!("native_reader={version}"),
                Err(error) => println!("native_reader_unavailable={error:#}"),
            }
            println!(
                "initial indexing reads snapshot ZAP/SA metadata; directory walking is disabled"
            );
            return Ok(None);
        }
        Some("--scan-summary") => {
            let target = std::env::args()
                .nth(2)
                .context("use: neutrasearch-helper --scan-summary MOUNT")?;
            let mount = find_local_mount(&target)?;
            let mut received = 0u64;
            let stats = dispatch_lane(&mount, &mut |_| received += 1)?;
            println!(
                "fs={} mount={} records={} emitted={} files={} dirs={} wall_ms={} detail={}",
                mount.fs.label(),
                mount.mountpoint.display(),
                stats.records,
                received,
                stats.files,
                stats.dirs,
                stats.wall_ms,
                stats.detail
            );
            return Ok(None);
        }
        Some("--build-index") => {
            build_index::run()?;
            return Ok(None);
        }

        _ => {}
    }

    Ok(Some(Options {
        serve_index,
        watch_mount,
    }))
}
