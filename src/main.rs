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
        /// Re-resolve locked images to the latest digest of their tag (default: all)
        #[arg(long, conflicts_with = "check")]
        update: bool,
        /// With --update: canonical keys of the images to refresh, as in pin-oci.lock
        #[arg(requires = "update", conflicts_with = "check", value_name = "IMAGE")]
        images: Vec<String>,
    },
}

/// Exit codes: 0 = in sync, 1 = out of sync, 2 = error.
fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Lock {
            flake, file, check, ..
        } if check => run_check(&flake, file.as_deref()),
        Command::Lock {
            flake,
            file,
            update,
            images,
            ..
        } => {
            let refresh = match (update, images.is_empty()) {
                (false, _) => lock::Refresh::Missing,
                (true, true) => lock::Refresh::All,
                (true, false) => lock::Refresh::Only(images),
            };
            run_lock(&flake, file.as_deref(), &refresh)
        }
    }
}

fn run_lock(flake: &str, file: Option<&Path>, refresh: &lock::Refresh) -> ExitCode {
    match lock::lock(&nix::NixCli, &registry::SkopeoCli, flake, file, refresh) {
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
            Command::Lock {
                flake, file, check, ..
            } => {
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
            Command::Lock {
                flake, file, check, ..
            } => {
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
            Command::Lock {
                flake, file, check, ..
            } => {
                assert_eq!(flake, ".#box");
                assert_eq!(file, Some(PathBuf::from("x.lock")));
                assert!(check);
            }
        }
    }

    #[test]
    fn update_takes_optional_canonical_keys() {
        match parse(&["lock", "--update"]).unwrap().command {
            Command::Lock { update, images, .. } => assert!(update && images.is_empty()),
        }
        match parse(&["lock", "--update", "docker.io/library/caddy:2-alpine", "b"])
            .unwrap()
            .command
        {
            Command::Lock { update, images, .. } => {
                assert!(update);
                assert_eq!(images, ["docker.io/library/caddy:2-alpine", "b"]);
            }
        }
    }

    #[test]
    fn images_need_update_and_update_excludes_check() {
        assert!(parse(&["lock", "docker.io/library/caddy:2-alpine"]).is_err());
        assert!(parse(&["lock", "--check", "docker.io/library/caddy:2-alpine"]).is_err());
        assert!(parse(&["lock", "--check", "--update"]).is_err());
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
