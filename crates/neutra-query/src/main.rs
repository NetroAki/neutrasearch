//! Scriptable Neutrasearch client. Opens the compact mmap index read-only and
//! never scans a filesystem. `--stdio` keeps one process alive for NDJSON RPC.
mod service;

use anyhow::{bail, Context, Result};
use service::{open_pair, run, serve};
use std::path::PathBuf;

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index_path = None;
    let mut limit = 50usize;
    let mut json = false;
    let mut json_paths = false;
    let mut stdio = false;
    let mut scope_roots = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--index" => {
                if i + 1 >= args.len() {
                    bail!("--index requires a path");
                }
                index_path = Some(PathBuf::from(args.remove(i + 1)));
                args.remove(i);
            }
            "--limit" => {
                if i + 1 >= args.len() {
                    bail!("--limit requires a number");
                }
                limit = args[i + 1]
                    .parse::<usize>()
                    .context("invalid --limit")?
                    .clamp(1, 1000);
                args.drain(i..=i + 1);
            }
            "--scope" => {
                if i + 1 >= args.len() {
                    bail!("--scope requires an absolute path");
                }
                let scope = PathBuf::from(args.remove(i + 1));
                if !scope.is_absolute() {
                    bail!("--scope requires an absolute path");
                }
                scope_roots.push(scope.to_string_lossy().into_owned());
                args.remove(i);
            }
            "--json" => {
                json = true;
                args.remove(i);
            }
            "--json-paths" => {
                json_paths = true;
                args.remove(i);
            }
            "--stdio" => {
                stdio = true;
                args.remove(i);
            }
            "--help" | "-h" => {
                println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--scope ROOT] [--limit N] [--json|--json-paths]");
                println!("Filters: ext:rs,toml  kind:file|dir|link  fs:btrfs|ext4|ntfs|zfs  size:>100M  size:1M..2M  under:/dir");
                println!("Internal persistent mode: neutrasearch-query --index INDEX.nsx --stdio");
                return Ok(());
            }
            x if x.starts_with('-') => bail!("unknown option {x}"),
            _ => i += 1,
        }
    }
    let path = neutra_core::paths::resolve_index_path(index_path);
    let (index, delta) = open_pair(&path)?;
    if stdio {
        return serve(
            &path,
            index,
            delta,
            std::io::stdin().lock(),
            std::io::stdout().lock(),
        );
    }
    if args.is_empty() {
        bail!("query is required (or use --stdio)");
    }
    let request = service::Request {
        query: args.join(" "),
        limit: Some(limit),
        metadata: Some(json),
        scope_roots,
        scope_case_sensitive: Some(cfg!(not(any(target_os = "windows", target_os = "macos")))),
    };
    let response = run(&index, delta.as_ref(), request)?;
    if json || json_paths {
        serde_json::to_writer(std::io::stdout().lock(), &response)?;
        println!();
    } else {
        for path in response.paths {
            println!("{path}");
        }
    }
    Ok(())
}

#[cfg(test)]
 mod tests {
     use super::*;
     use neutra_core::{CompactIndex, FileKind, FileRecord, FsKind};

     fn build_test_base(records: &[FileRecord], path: &std::path::Path) {
         let mut spill = neutra_core::SpillAccumulator::begin(path).unwrap();
         spill.push_batch(records.to_vec()).unwrap();
         CompactIndex::rebuild_streamed(spill.finish().unwrap(), path).unwrap();
     }
    #[test]
    fn ndjson_api() {
        let path =
            std::env::temp_dir().join(format!("neutra-query-test-{}.nsx", std::process::id()));
        let (allowed_file, private_file, allowed_root) = if cfg!(target_os = "windows") {
            ("C:/src/needle.rs", "C:/private/needle-key.txt", "C:/src")
        } else {
            ("/src/needle.rs", "/private/needle-key.txt", "/src")
        };
        let records = vec![
            FileRecord {
                path: allowed_file.into(),
                size: 7,
                mtime: 1,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ext4,
                native_id: 1,
                native_parent: 2,
                source: 0,
                disk: 0,
            },
            FileRecord {
                path: private_file.into(),
                size: 9,
                mtime: 2,
                mode: 0,
                kind: FileKind::File,
                fs: FsKind::Ext4,
                native_id: 2,
                native_parent: 3,
                source: 0,
                disk: 0,
            },
        ];
         build_test_base(&records, &path);
         let index = CompactIndex::open(&path).unwrap();
        let mut output = Vec::new();
        let request = format!(
            "{{\"query\":\"needle\",\"limit\":5,\"scope_roots\":[{allowed_root:?}],\"scope_case_sensitive\":true}}\n"
        );
        serve(&path, index, None, request.as_bytes(), &mut output).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["paths"], serde_json::json!([allowed_file]));

        let index = CompactIndex::open(&path).unwrap();
        let error = match run(
            &index,
            None,
            service::Request {
                query: "needle".into(),
                limit: Some(5),
                metadata: Some(false),
                scope_roots: vec!["relative/path".into()],
                scope_case_sensitive: Some(true),
            },
        ) {
            Err(error) => error,
            Ok(_) => panic!("relative trusted scope must be rejected"),
        };
        assert!(error.to_string().contains("absolute paths"));
        std::fs::remove_file(path).unwrap();
    }
}
