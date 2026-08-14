use neutra_core::proto::HelperMsg;
use neutra_core::{CompactIndex, Index};
use std::path::{Path, PathBuf};
use std::process::Command;

pub enum Action {
    Gui,
    Exit(i32),
}

pub fn action() -> Action {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.is_empty() {
        return Action::Gui;
    }
    let command = args.remove(0);
    let Some(command) = command.to_str() else {
        eprintln!("neutrasearch: command must be valid UTF-8");
        return Action::Exit(2);
    };
    match command {
        "gui" => {
            if args.is_empty() {
                Action::Gui
            } else {
                usage_error("gui does not accept arguments")
            }
        }
        "search" => Action::Exit(search(args)),
        "index" => Action::Exit(index(args)),
        "serve" => Action::Exit(serve(args)),
        "mcp" => Action::Exit(with_index(args, "mcp", |index| {
            run_companion(
                "NEUTRASEARCH_MCP",
                "neutrasearch-mcp",
                Vec::new(),
                Some(("NEUTRASEARCH_INDEX", index)),
            )
        })),
        "help" | "--help" | "-h" => {
            print_help();
            Action::Exit(0)
        }
        "version" | "--version" | "-V" => {
            println!("neutrasearch {}", env!("CARGO_PKG_VERSION"));
            Action::Exit(0)
        }
        other => usage_error(&format!("unknown command '{other}'")),
    }
}

fn search(args: Vec<std::ffi::OsString>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--scope ROOT] [--limit N] [--json|--json-paths]");
        return 0;
    }
    if args.is_empty() {
        return error("search requires a query");
    }
    let explicit = match index_override(&args) {
        Ok(path) => path,
        Err(message) => return error(&message),
    };
    let path = neutra_core::paths::resolve_index_path(explicit);
    if let Err(message) = ensure_index(&path) {
        return error(&message);
    }
    run_companion("NEUTRASEARCH_QUERY", "neutrasearch-query", args, None)
}

fn index(args: Vec<std::ffi::OsString>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch index [--output INDEX.nsx]");
        return 0;
    }
    let output = match parse_index(args) {
        Ok(path) => neutra_core::paths::resolve_index_path(path),
        Err(message) => return error(&message),
    };
    match build_machine_index_on_large_stack(output) {
        Ok(()) => 0,
        Err(message) => error(&message),
    }
}

fn build_machine_index_on_large_stack(output: PathBuf) -> Result<(), String> {
    let worker = std::thread::Builder::new()
        .name("neutrasearch-index".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || build_machine_index(&output))
        .map_err(|error| format!("cannot start index builder: {error}"))?;
    worker
        .join()
        .map_err(|_| "the index builder overflowed its stack".to_string())?
}

fn parse_index(mut args: Vec<std::ffi::OsString>) -> Result<Option<PathBuf>, String> {
    let mut output = None;
    while !args.is_empty() {
        let option = args.remove(0);
        if option == "--output" || option == "-o" {
            if args.is_empty() {
                return Err("--output requires a path".into());
            }
            if output.is_some() {
                return Err("--output may be specified only once".into());
            }
            output = Some(PathBuf::from(args.remove(0)));
        } else {
            return Err(format!(
                "unknown index option {}; indexing always covers the full machine",
                option.to_string_lossy()
            ));
        }
    }
    Ok(output)
}

fn index_override(args: &[std::ffi::OsString]) -> Result<Option<PathBuf>, String> {
    let mut index = None;
    let mut position = 0;
    while position < args.len() {
        if args[position] == "--index" {
            let value = args
                .get(position + 1)
                .ok_or_else(|| "--index requires a path".to_string())?;
            if index.replace(PathBuf::from(value)).is_some() {
                return Err("--index may be specified only once".into());
            }
            position += 2;
        } else {
            position += 1;
        }
    }
    Ok(index)
}

fn ensure_index(path: &Path) -> Result<(), String> {
    match CompactIndex::open_with_delta_snapshot(path) {
        Ok((index, _delta)) => {
            drop(index);
            neutra_core::paths::remember_index_path(path)
                .map_err(|error| format!("cannot remember {}: {error}", path.display()))?;
            return Ok(());
        }
        Err(error) if path.exists() => {
            eprintln!(
                "neutrasearch: index at {} is not usable ({error}); indexing the full machine first",
                path.display()
            );
        }
        Err(_) => {
            eprintln!(
                "neutrasearch: no usable index exists at {}; indexing the full machine first",
                path.display()
            );
        }
    }
    build_machine_index(path)
}

fn build_machine_index(output: &Path) -> Result<(), String> {
    let roots = super::default_system_roots();
    let mounts = super::selected_scan_mounts(&roots);
    if mounts.is_empty() {
        return Err("no supported local native filesystems were discovered".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    super::spawn_local_helper(tx, cfg!(target_os = "linux"), mounts, roots.clone());
    let mut staging = Index::new();
    let (completed_mounts, errors) = loop {
        match rx.recv() {
            Ok(super::Event::Message(HelperMsg::ScanBegin { mount })) => {
                eprintln!("neutrasearch: indexing {}", mount.mountpoint.display());
            }
            Ok(super::Event::Message(HelperMsg::Records(records))) => {
                staging.extend(
                    records
                        .into_iter()
                        .filter(|record| super::record_in_roots(record.path.as_ref(), &roots)),
                );
            }
            Ok(super::Event::Message(HelperMsg::ScanDone { mount, stats })) => {
                eprintln!(
                    "neutrasearch: indexed {} records from {} in {} ms",
                    stats.records,
                    mount.mountpoint.display(),
                    stats.wall_ms
                );
            }
            Ok(super::Event::Message(HelperMsg::ScanError { mount, error })) => {
                eprintln!(
                    "neutrasearch: skipped {}: {error}",
                    mount.mountpoint.display()
                );
            }
            Ok(super::Event::Message(HelperMsg::ScanComplete { mounts, errors })) => {
                break (mounts, errors);
            }
            Ok(super::Event::Message(HelperMsg::Error(error))) => return Err(error),
            Ok(super::Event::Fatal(error)) => return Err(error),
            Ok(_) => {}
            Err(_) => return Err("native scanner stopped before completing".into()),
        }
    };
    if !super::scan_has_reachable_lane(completed_mounts, errors) {
        return Err(format!(
            "no native filesystem completed successfully ({errors} error(s))"
        ));
    }
    if staging.is_empty() {
        return Err("native scanners returned no files; the previous index was kept".into());
    }
    let built = CompactIndex::rebuild(staging.records(), output)
        .map_err(|error| format!("cannot publish {}: {error}", output.display()))?;
    let remembered = neutra_core::paths::remember_index_path(output)
        .map_err(|error| format!("cannot remember {}: {error}", output.display()))?;
    println!(
        "indexed={} bytes={} build_ms={} output={}",
        built.records,
        built.bytes,
        built.wall_ms,
        remembered.display()
    );
    Ok(())
}

fn serve(args: Vec<std::ffi::OsString>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch serve [--index INDEX.nsx] [--watch MOUNT] [--source ID]");
        println!("\nLinux watch mode requires CAP_SYS_ADMIN and CAP_DAC_READ_SEARCH (or root).");
        return 0;
    }
    let (index, watch, source) = match parse_serve(args) {
        Ok(config) => config,
        Err(message) => return error(&message),
    };
    if let Err(message) = ensure_index(&index) {
        return error(&message);
    }
    let helper_args = if let Some(mount) = watch {
        vec![
            "--watch-index".into(),
            index.into_os_string(),
            mount.into_os_string(),
            source.to_string().into(),
        ]
    } else {
        vec!["--serve-index".into(), index.into_os_string()]
    };
    run_companion(
        "NEUTRASEARCH_HELPER",
        "neutrasearch-helper",
        helper_args,
        None,
    )
}

fn parse_serve(
    mut args: Vec<std::ffi::OsString>,
) -> Result<(PathBuf, Option<PathBuf>, u32), String> {
    let mut index = None;
    let mut watch = None;
    let mut source = None;
    while !args.is_empty() {
        let option = args.remove(0);
        if option == "--index" || option == "--watch" || option == "--source" {
            if args.is_empty() {
                return Err(format!("{} requires a value", option.to_string_lossy()));
            }
            let value = args.remove(0);
            if option == "--index" {
                index = Some(PathBuf::from(value));
            } else if option == "--watch" {
                watch = Some(PathBuf::from(value));
            } else {
                source = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--source requires an unsigned integer".to_string())?,
                );
            }
        } else {
            return Err(format!("unknown serve option {}", option.to_string_lossy()));
        }
    }
    let index = neutra_core::paths::resolve_index_path(index);
    if source.is_some() && watch.is_none() {
        return Err("--source requires --watch MOUNT".into());
    }
    Ok((index, watch, source.unwrap_or(0)))
}

fn with_index(
    mut args: Vec<std::ffi::OsString>,
    command: &str,
    run: impl FnOnce(PathBuf) -> i32,
) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch {command} [--index INDEX.nsx]");
        return 0;
    }
    let explicit = match args.len() {
        0 => None,
        2 if args[0] == "--index" => Some(PathBuf::from(args.remove(1))),
        _ => return error(&format!("{command} accepts only [--index INDEX.nsx]")),
    };
    let index = neutra_core::paths::resolve_index_path(explicit);
    if let Err(message) = ensure_index(&index) {
        return error(&message);
    }
    run(index)
}

fn run_companion(
    env_name: &str,
    binary: &str,
    args: Vec<std::ffi::OsString>,
    environment: Option<(&str, PathBuf)>,
) -> i32 {
    let program = companion(env_name, binary);
    let mut command = Command::new(&program);
    command.args(args);
    if let Some((name, value)) = environment {
        command.env(name, value);
    }
    match command.status() {
        Ok(status) => status.code().unwrap_or(1),
        Err(error) => {
            eprintln!(
                "neutrasearch: could not start {}: {error}",
                program.display()
            );
            1
        }
    }
}

fn companion(env_name: &str, binary: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env_name) {
        return path.into();
    }
    if let Ok(current) = std::env::current_exe() {
        let candidate = current.with_file_name(executable_name(binary));
        if candidate.is_file() {
            return candidate;
        }
    }
    executable_name(binary).into()
}

fn executable_name(binary: &str) -> String {
    if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    }
}

fn usage_error(message: &str) -> Action {
    eprintln!("neutrasearch: {message}\nRun 'neutrasearch help' for usage.");
    Action::Exit(2)
}

fn error(message: &str) -> i32 {
    eprintln!("neutrasearch: {message}");
    2
}

fn print_help() {
    println!(
        "Neutrasearch — fast indexed filename search\n\n\
Usage:\n  \
  neutrasearch [gui]\n  \
  neutrasearch search QUERY [--index INDEX.nsx] [--scope ROOT] [--limit N] [--json|--json-paths]\n  \
  neutrasearch index [--output INDEX.nsx]\n  \
  neutrasearch serve [--index INDEX.nsx] [--watch MOUNT]\n  \
  neutrasearch mcp [--index INDEX.nsx]\n\n\
Commands:\n  \
  gui      Open the desktop application (default)\n  \
  search   Search the last index, building it when missing\n  \
  index    Fully index all supported local filesystems\n  \
  serve    Run the framed index service on stdin/stdout\n  \
  mcp      Run the MCP server on stdin/stdout"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn companion_uses_product_prefix() {
        assert_eq!(
            executable_name("neutrasearch-query"),
            if cfg!(windows) {
                "neutrasearch-query.exe"
            } else {
                "neutrasearch-query"
            }
        );
    }

    #[test]
    fn index_command_defaults_to_the_machine_index_location() {
        assert_eq!(parse_index(Vec::new()).unwrap(), None);
        assert_eq!(
            parse_index(vec!["--output".into(), "files.nsx".into()]).unwrap(),
            Some(PathBuf::from("files.nsx"))
        );
    }

    #[test]
    fn index_command_has_no_root_or_depth_mode() {
        assert!(parse_index(vec!["/mnt/data".into()])
            .unwrap_err()
            .contains("full machine"));
        assert!(parse_index(vec!["--depth".into(), "2".into()])
            .unwrap_err()
            .contains("full machine"));
    }

    #[test]
    fn serve_watch_has_explicit_source() {
        let (index, watch, source) = parse_serve(vec![
            "--index".into(),
            "files.nsx".into(),
            "--watch".into(),
            "/home".into(),
            "--source".into(),
            "4".into(),
        ])
        .unwrap();
        assert_eq!(index, PathBuf::from("files.nsx"));
        assert_eq!(watch, Some(PathBuf::from("/home")));
        assert_eq!(source, 4);
    }
}
