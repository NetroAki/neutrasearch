use std::path::PathBuf;
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
        "serve" => Action::Exit(with_index(args, "serve", |index| {
            run_companion(
                "NEUTRASEARCH_HELPER",
                "neutrasearch-helper",
                vec!["--serve-index".into(), index.into_os_string()],
                None,
            )
        })),
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
        println!("Usage: neutrasearch search QUERY [--index INDEX.nsx] [--limit N] [--json]");
        return 0;
    }
    if args.is_empty() {
        return error("search requires a query");
    }
    run_companion("NEUTRASEARCH_QUERY", "neutrasearch-query", args, None)
}

fn index(args: Vec<std::ffi::OsString>) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch index MOUNT --output INDEX.nsx");
        return 0;
    }
    let (mount, output) = match parse_index(args) {
        Ok(paths) => paths,
        Err(message) => return error(&message),
    };
    run_companion(
        "NEUTRASEARCH_HELPER",
        "neutrasearch-helper",
        vec![
            "--build-index".into(),
            mount.into_os_string(),
            output.into_os_string(),
        ],
        None,
    )
}

fn parse_index(mut args: Vec<std::ffi::OsString>) -> Result<(PathBuf, PathBuf), String> {
    let mut mount = None;
    let mut output = None;
    while !args.is_empty() {
        if args[0] == "--output" || args[0] == "-o" {
            if args.len() < 2 {
                return Err("--output requires a path".into());
            }
            output = Some(PathBuf::from(args.remove(1)));
            args.remove(0);
        } else if args[0].to_string_lossy().starts_with('-') {
            return Err(format!(
                "unknown index option {}",
                args[0].to_string_lossy()
            ));
        } else if mount.is_none() {
            mount = Some(PathBuf::from(args.remove(0)));
        } else {
            return Err("index accepts one mount point".into());
        }
    }
    let mount = mount.ok_or_else(|| "index requires a mount point".to_string())?;
    let output = output.ok_or_else(|| "index requires --output INDEX.nsx".to_string())?;
    Ok((mount, output))
}

fn with_index(
    mut args: Vec<std::ffi::OsString>,
    command: &str,
    run: impl FnOnce(PathBuf) -> i32,
) -> i32 {
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("Usage: neutrasearch {command} --index INDEX.nsx");
        return 0;
    }
    if args.len() != 2 || args[0] != "--index" {
        return error(&format!("{command} requires exactly --index INDEX.nsx"));
    }
    run(PathBuf::from(args.remove(1)))
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
  neutrasearch search QUERY [--index INDEX.nsx] [--limit N] [--json]\n  \
  neutrasearch index MOUNT --output INDEX.nsx\n  \
  neutrasearch serve --index INDEX.nsx\n  \
  neutrasearch mcp --index INDEX.nsx\n\n\
Commands:\n  \
  gui      Open the desktop application (default)\n  \
  search   Search an existing index\n  \
  index    Build an index from one mounted native filesystem\n  \
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
    fn index_command_accepts_human_ordering() {
        let (mount, output) = parse_index(vec![
            "/mnt/data".into(),
            "--output".into(),
            "files.nsx".into(),
        ])
        .unwrap();
        assert_eq!(mount, PathBuf::from("/mnt/data"));
        assert_eq!(output, PathBuf::from("files.nsx"));
    }

    #[test]
    fn index_command_explains_missing_output() {
        assert_eq!(
            parse_index(vec!["/mnt/data".into()]).unwrap_err(),
            "index requires --output INDEX.nsx"
        );
    }
}
