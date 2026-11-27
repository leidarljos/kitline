//! Stamp a message with `Kitline-Change`, or read one back from a commit.

use clap::Parser;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "kitline stamp",
    about = "Print a message carrying Kitline-Change"
)]
struct StampCli {
    /// Message text. Otherwise the message is read from -F or stdin.
    #[arg(short, long)]
    message: Option<String>,
    /// Read the message from a file.
    #[arg(short = 'F', long)]
    file: Option<PathBuf>,
}

#[derive(Parser)]
#[command(name = "kitline track", about = "Print the Kitline-Change on a commit")]
struct TrackCli {
    /// Commit to read. Defaults to HEAD.
    rev: Option<String>,
    /// Repository. Defaults to the current directory.
    #[arg(long)]
    repo: Option<PathBuf>,
}

pub fn run_stamp() -> i32 {
    let mut argv = vec!["kitline stamp".to_string()];
    match utf8_args(2) {
        Ok(rest) => argv.extend(rest),
        Err(err) => {
            eprintln!("stamp: {err}");
            return 1;
        }
    }
    let cli = StampCli::parse_from(argv);
    match stamped(&cli) {
        Ok(text) => {
            print!("{text}");
            0
        }
        Err(err) => {
            eprintln!("stamp: {err}");
            if err.contains("pass one") {
                2
            } else {
                1
            }
        }
    }
}

pub fn run_track() -> i32 {
    let mut argv = vec!["kitline track".to_string()];
    match utf8_args(2) {
        Ok(rest) => argv.extend(rest),
        Err(err) => {
            eprintln!("track: {err}");
            return 1;
        }
    }
    let cli = TrackCli::parse_from(argv);
    let repo = cli.repo.unwrap_or_else(|| PathBuf::from("."));
    let rev = cli.rev.unwrap_or_else(|| "HEAD".to_string());
    let out = crate::gitutil::git(&repo, &["cat-file", "commit", "--end-of-options", &rev]);
    if out.status != 0 {
        eprintln!("track: {}", out.stderr.trim());
        return 1;
    }
    match kitline_core::read_commit(&out.stdout) {
        Ok(Some(id)) => {
            println!("{id}");
            0
        }
        Ok(None) => {
            eprintln!("track: no Kitline-Change");
            1
        }
        Err(err) => {
            eprintln!("track: {err}");
            1
        }
    }
}

fn utf8_args(skip: usize) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for arg in std::env::args_os().skip(skip) {
        match arg.to_str() {
            Some(text) => out.push(text.to_string()),
            None => return Err("argument is not utf-8".into()),
        }
    }
    Ok(out)
}

fn stamped(cli: &StampCli) -> Result<String, String> {
    let message = match (&cli.message, &cli.file) {
        (Some(_), Some(_)) => return Err("pass one of -m and -F".into()),
        (Some(text), None) => text.clone(),
        (None, Some(path)) => {
            fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?
        }
        (None, None) => {
            let mut buf = String::new();
            io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| format!("stdin: {e}"))?;
            buf
        }
    };
    let id = kitline_core::mint();
    let stamped = kitline_core::stamp(&message, &id).map_err(|e| e.to_string())?;
    Ok(stamped.message)
}
