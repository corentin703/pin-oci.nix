mod check;
mod lockfile;
mod nix;
#[cfg(test)]
mod testutil;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "pin-oci", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Resolve declared images to per-platform digests and write pin-oci.lock
    Lock {
        /// Flake reference to evaluate
        #[arg(long, default_value = ".")]
        flake: String,
        /// Lockfile to write (default: <flake root>/pin-oci.lock)
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Check that every declared image is present in the lockfile
    Check {
        /// Flake reference to evaluate, as <flake>#<host>
        #[arg(long, default_value = ".")]
        flake: String,
        /// Lockfile to read (default: <flake root>/pin-oci.lock)
        #[arg(long)]
        file: Option<PathBuf>,
    },
}

/// Exit codes: 0 = in sync, 1 = out of sync, 2 = error.
fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Lock { .. } => {
            eprintln!("pin-oci lock: not implemented");
            ExitCode::from(2)
        }
        Command::Check { flake, file } => run_check(&flake, file.as_deref()),
    }
}

fn run_check(flake: &str, file: Option<&Path>) -> ExitCode {
    match check::check(&nix::NixCli, flake, file) {
        Ok(report) if report.is_in_sync() => {
            println!("{}", report.message());
            ExitCode::SUCCESS
        }
        Ok(report) => {
            eprintln!("{}", report.message());
            ExitCode::from(1)
        }
        Err(e) => {
            eprintln!("pin-oci: error: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("pin-oci").chain(args.iter().copied()))
    }

    #[test]
    fn lock_defaults() {
        match parse(&["lock"]).unwrap().command {
            Command::Lock { flake, file } => {
                assert_eq!(flake, ".");
                assert_eq!(file, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn lock_with_flake_and_file() {
        match parse(&["lock", "--flake", "/etc/nixos#host", "--file", "x.lock"])
            .unwrap()
            .command
        {
            Command::Lock { flake, file } => {
                assert_eq!(flake, "/etc/nixos#host");
                assert_eq!(file, Some(PathBuf::from("x.lock")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn check_defaults() {
        match parse(&["check"]).unwrap().command {
            Command::Check { flake, file } => {
                assert_eq!(flake, ".");
                assert_eq!(file, None);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn check_with_flake_and_file() {
        match parse(&["check", "--flake", ".#box", "--file", "x.lock"])
            .unwrap()
            .command
        {
            Command::Check { flake, file } => {
                assert_eq!(flake, ".#box");
                assert_eq!(file, Some(PathBuf::from("x.lock")));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        assert!(parse(&["bogus"]).is_err());
    }

    #[test]
    fn missing_subcommand_is_rejected() {
        assert!(parse(&[]).is_err());
    }
}
