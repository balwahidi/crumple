use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Local-first batch image optimizer (pre-alpha: nothing works yet).
#[derive(Parser, Debug)]
#[command(name = "crumple", version, about)]
struct Cli {
    /// Image file or folder to optimize.
    path: PathBuf,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let _ = cli.path;
    eprintln!("not implemented yet");
    ExitCode::from(2)
}
