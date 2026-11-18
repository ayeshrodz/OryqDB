#![forbid(unsafe_code)]

//! Workspace verification and architecture boundary enforcement CLI (`cargo run -p xtask -- ...`).

mod architecture;
mod fixtures;
mod header;
mod module_checks;

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        print_usage();
        return ExitCode::FAILURE;
    }

    let root = workspace_root();
    let result = match args[0].as_str() {
        "architecture" => run_architecture(&root, &args[1..]),
        "modules" => {
            if args.len() != 2 {
                Err("usage: cargo run -p xtask -- modules <module-id>".to_string())
            } else {
                module_checks::run_module_checks(&root, &args[1])
            }
        }
        "header" => header::check_header(&root),
        "fixtures" => fixtures::check_fixtures(&root),
        "-h" | "--help" | "help" => {
            print_usage();
            Ok(())
        }
        other => Err(format!("unknown xtask subcommand `{other}`")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("xtask error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_architecture(root: &std::path::Path, args: &[String]) -> Result<(), String> {
    let mut base_ref: Option<String> = None;
    let mut changed_paths_json: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--base" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--base requires a git ref argument".to_string())?;
                base_ref = Some(value.clone());
            }
            "--changed-paths" => {
                i += 1;
                let value = args
                    .get(i)
                    .ok_or_else(|| "--changed-paths requires a JSON file path".to_string())?;
                changed_paths_json = Some(PathBuf::from(value));
            }
            flag => return Err(format!("unknown architecture flag `{flag}`")),
        }
        i += 1;
    }

    if base_ref.is_some() && changed_paths_json.is_some() {
        return Err("cannot specify both --base and --changed-paths".to_string());
    }

    architecture::run_cli(root, base_ref.as_deref(), changed_paths_json.as_deref())
}

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("xtask lives directly under workspace root")
        .to_path_buf()
}

fn print_usage() {
    println!(
        "Usage:\n  \
         cargo run -p xtask -- architecture [--base <git-ref> | --changed-paths <json-file>]\n  \
         cargo run -p xtask -- modules <module-id>\n  \
         cargo run -p xtask -- header\n  \
         cargo run -p xtask -- fixtures"
    );
}
