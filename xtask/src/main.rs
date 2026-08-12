//! Maintainer tasks for this workspace.
//!
//! Run with `cargo xtask <task>`. Nothing here is needed to build or test the crates; these are
//! the jobs that keep the committed registry data in step with the specification it comes from.

mod generate;
mod markdown;
mod registry;

use std::path::{Path, PathBuf};

const USAGE: &str = "\
cargo xtask <task>

Tasks:
  registries [--docs <path>]   Regenerate the registry tables.

Without --docs, the tables are rebuilt from the JSON this repository already carries, which is
what CI runs: it checks the committed tables still match their data, and needs nothing fetched.

With --docs pointing at a checkout of the specifications repository, the JSON is re-read from the
registry pages first, so a change made there lands in the JSON and then in the tables.
";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("registries") => registries(&args[1..]),
        Some("-h" | "--help" | "help") | None => {
            print!("{USAGE}");
            Ok(())
        }
        Some(other) => Err(format!("unknown task {other:?}\n\n{USAGE}")),
    }
}

/// The workspace root, found from this crate rather than from the working directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("xtask sits in the workspace").to_path_buf()
}

fn registries(args: &[String]) -> Result<(), String> {
    let docs = match args {
        [] => None,
        [flag, path] if flag == "--docs" => Some(PathBuf::from(path)),
        _ => return Err(format!("expected `registries [--docs <path>]`\n\n{USAGE}")),
    };

    let root = workspace_root();
    let data_dir = root.join("crates/one-saves-registry/data");
    let generated = root.join("crates/one-saves-registry/src/generated.rs");

    let data = match &docs {
        Some(docs) => {
            if !docs.join("src/content/docs").is_dir() {
                return Err(format!("{} does not look like the specifications repository", docs.display()));
            }
            let data = registry::read_from_docs(docs)?;
            registry::write_json(&data, &data_dir)?;
            println!("synced {} from {}", data_dir.display(), docs.display());
            data
        }
        None => registry::read_json(&data_dir)?,
    };

    generate::write_rust(&data, &generated)?;
    println!(
        "wrote {} ({} systems, {} cores, {} roles, {} prefixes, {} card formats, {} device kinds, \
         {} bindings, {} vendors)",
        generated.display(),
        data.systems.len(),
        data.cores.len(),
        data.roles.roles.len(),
        data.roles.prefixes.len(),
        data.card_formats.len(),
        data.device_kinds.len(),
        data.bindings.len(),
        data.vendors.len(),
    );
    Ok(())
}
