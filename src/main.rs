mod check;
mod lock;
mod lockfile;
mod nix;
mod registry;
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
        /// Flake reference to evaluate, as <flake>#<host>
        #[arg(long, default_value = ".")]
        flake: String,
        /// Lockfile to read or write (default: <flake root>/pin-oci.lock)
        #[arg(long)]
        file: Option<PathBuf>,
        /// Only check that every declared image is in the lockfile; change nothing
        #[arg(long)]
        check: bool,
    },
}

/// Exit codes: 0 = in sync, 1 = out of sync, 2 = error.
fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Lock { flake, file, check } if check => run_check(&flake, file.as_deref()),
        Command::Lock { flake, file, .. } => run_lock(&flake, file.as_deref()),
    }
}

fn run_lock(flake: &str, file: Option<&Path>) -> ExitCode {
    match lock::lock(&nix::NixCli, &registry::SkopeoCli, flake, file) {
        Ok(outcome) => {
            println!("{}", outcome.message());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pin-oci: error: {e}");
            ExitCode::from(2)
        }
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
            Command::Lock { flake, file, check } => {
                assert_eq!(flake, ".");
                assert_eq!(file, None);
                assert!(!check);
            }
        }
    }

    #[test]
    fn lock_with_flake_and_file() {
        match parse(&["lock", "--flake", "/etc/nixos#host", "--file", "x.lock"])
            .unwrap()
            .command
        {
            Command::Lock { flake, file, check } => {
                assert_eq!(flake, "/etc/nixos#host");
                assert_eq!(file, Some(PathBuf::from("x.lock")));
                assert!(!check);
            }
        }
    }

    #[test]
    fn lock_check_with_flake_and_file() {
        match parse(&["lock", "--check", "--flake", ".#box", "--file", "x.lock"])
            .unwrap()
            .command
        {
            Command::Lock { flake, file, check } => {
                assert_eq!(flake, ".#box");
                assert_eq!(file, Some(PathBuf::from("x.lock")));
                assert!(check);
            }
        }
    }

    #[test]
    fn check_is_no_longer_a_subcommand() {
        assert!(parse(&["check"]).is_err());
        assert!(parse(&["check", "--flake", ".#box"]).is_err());
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
