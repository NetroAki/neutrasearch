use super::domain;
use neutra_core::CompactIndex;
use std::path::PathBuf;
use std::process::Command;

pub enum Action {
    Gui,
    Spotlight,
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
        "--spotlight" if args.is_empty() => Action::Spotlight,
        "gui" => {
            if args.is_empty() {
                Action::Gui
            } else {
                usage_error("gui does not accept arguments")
            }
        }
        "search" => Action::Exit(search(args)),
        "index" => Action::Exit(index(args)),
        "--ensure-index" if args.is_empty() => {
            let path = neutra_core::paths::resolve_index_path(None);
            let result = if path.exists() {
                CompactIndex::open_fast(&path)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            } else {
                domain::ensure_index(&path)
            };
            Action::Exit(match result {
                Ok(()) => 0,
                Err(message) => error(&message),
            })
        }
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
    let no_build = args.iter().any(|arg| arg == "--no-build");
    let args = args
        .into_iter()
        .filter(|arg| arg != "--no-build")
        .collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--scope ROOT] [--limit N] [--json|--json-paths] [--no-build]");
        println!("Filters: ext:rs,toml  kind:file|dir|link  fs:btrfs|ext4|ntfs|zfs  size:>100M  size:1M..2M  under:/dir");
        println!("\"quoted phrase\" groups words; without --no-build a missing index is built first (may need administrator access).");
        return 0;
    }
    if args.is_empty() {
        return error("search requires a query");
    }
    let explicit = match domain::index_override(&args) {
        Ok(path) => path,
        Err(message) => return error(&message),
    };
    let path = neutra_core::paths::resolve_index_path(explicit);
    if no_build {
        if let Err(message) = domain::ensure_index_without_build(&path) {
            return error(&message);
        }
    } else if let Err(message) = domain::ensure_index(&path) {
        return error(&message);
    }
    run_companion("NEUTRASEARCH_QUERY", "neutrasearch-query", args, None)
}

fn index(args: Vec<std::ffi::OsString>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch index [--output INDEX.nsx]");
        return 0;
    }
    let allow_zfs_enumerate = args.iter().any(|arg| arg == "--zfs-enumerate");
    if allow_zfs_enumerate {
        return error("Directory walking is disabled; ZFS requires a native metadata backend");
    }
    let args = args
        .into_iter()
        .filter(|arg| arg != "--zfs-enumerate")
        .collect::<Vec<_>>();
    let output = match domain::parse_index(args) {
        Ok(path) => neutra_core::paths::resolve_index_path(path),
        Err(message) => return error(&message),
    };
    match domain::build_machine_index_on_large_stack(output.clone(), allow_zfs_enumerate) {
        Ok(()) => {
            domain::build_ranked_lists(&output);
            0
        }
        Err(message) => error(&message),
    }
}

/// Precompute the newest and largest listings so the GUI's first screen is
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
    if let Err(message) = domain::ensure_index(&index) {
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

pub(super) fn parse_serve(
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
    if let Err(message) = domain::ensure_index(&index) {
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

pub(super) fn executable_name(binary: &str) -> String {
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
        "Neutrasearch: fast indexed filename search\n\n\
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
