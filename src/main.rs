use std::path::PathBuf;
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
        /// Flake reference to evaluate
        #[arg(long, default_value = ".")]
        flake: String,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = match cli.command {
        Command::Lock { .. } => "lock",
        Command::Check { .. } => "check",
    };
    eprintln!("pin-oci {name}: not implemented");
    ExitCode::FAILURE
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
            Command::Check { flake } => assert_eq!(flake, "."),
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn check_rejects_file_flag() {
        assert!(parse(&["check", "--file", "x.lock"]).is_err());
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
