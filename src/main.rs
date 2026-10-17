mod check;
mod convert;
mod list;
mod lock;
mod lockfile;
mod nix;
mod registry;
mod render;
#[cfg(test)]
mod testutil;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

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
        /// List declared and locked images with their status for the host platform
        #[arg(long, conflicts_with_all = ["check", "update"])]
        list: bool,
        /// Re-resolve locked images to the latest digest of their tag (default: all)
        #[arg(long, conflicts_with = "check")]
        update: bool,
        /// With --update: canonical keys of the images to refresh, as in pin-oci.lock
        #[arg(requires = "update", conflicts_with_all = ["check", "list"], value_name = "IMAGE")]
        images: Vec<String>,
    },
    /// Convert a docker-compose file (a useful subset) into a pin-oci project module
    Convert {
        /// The compose file to read
        file: PathBuf,
        /// Project name (default: the file's `name:`, else its directory name)
        #[arg(long)]
        name: Option<String>,
        /// Write the module to this file instead of stdout
        #[arg(long)]
        output: Option<PathBuf>,
        /// Fail (exit 1, write nothing) if anything in the file could not be converted
        #[arg(long)]
        strict: bool,
    },
    /// Print a shell completion script to stdout
    Completions {
        /// Shell to generate the script for
        shell: Shell,
    },
}

/// Exit codes: 0 = success (`--check`: in sync), 1 = `--check` out of sync or `convert --strict`
/// with warnings, 2 = error.
fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Lock {
            flake, file, check, ..
        } if check => run_check(&flake, file.as_deref()),
        Command::Lock {
            flake, file, list, ..
        } if list => run_list(&flake, file.as_deref()),
        Command::Convert {
            file,
            name,
            output,
            strict,
        } => run_convert(&file, name.as_deref(), output.as_deref(), strict),
        Command::Completions { shell } => {
            completions(shell, &mut std::io::stdout());
            ExitCode::SUCCESS
        }
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

/// Exit codes: 0 = converted, 1 = `--strict` and something was not converted, 2 = error.
fn run_convert(file: &Path, name: Option<&str>, output: Option<&Path>, strict: bool) -> ExitCode {
    let fail = |message: String| {
        eprintln!("pin-oci: error: {message}");
        ExitCode::from(2)
    };
    let yaml = match std::fs::read_to_string(file) {
        Ok(yaml) => yaml,
        Err(e) => return fail(format!("cannot read {}: {e}", file.display())),
    };
    let directory = std::fs::canonicalize(file).ok().and_then(|path| {
        path.parent()?
            .file_name()
            .map(|d| d.to_string_lossy().into_owned())
    });
    // Only the file name goes in the header: the output must not depend on where it was run.
    let source = file.file_name().map_or_else(
        || file.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let converted = match convert::convert(&yaml, name, directory.as_deref(), &source) {
        Ok(converted) => converted,
        Err(e) => return fail(format!("{}: {e}", file.display())),
    };
    for warning in &converted.warnings {
        eprintln!("pin-oci: warning: {warning}");
    }
    if strict && !converted.warnings.is_empty() {
        eprintln!(
            "pin-oci: --strict: {} warning(s), nothing written",
            converted.warnings.len()
        );
        return ExitCode::from(1);
    }
    match output {
        None => print!("{}", converted.nix),
        Some(path) => {
            if let Err(e) = std::fs::write(path, &converted.nix) {
                return fail(format!("cannot write {}: {e}", path.display()));
            }
            println!(
                "pin-oci: wrote {} ({} warning(s))",
                path.display(),
                converted.warnings.len()
            );
        }
    }
    ExitCode::SUCCESS
}

/// The script is derived from the real command definition, so it cannot drift from the CLI.
fn completions(shell: Shell, out: &mut dyn Write) {
    clap_complete::generate(shell, &mut Cli::command(), "pin-oci", out);
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

fn run_list(flake: &str, file: Option<&Path>) -> ExitCode {
    match list::list(&nix::NixCli, flake, file) {
        Ok(listing) => {
            println!("{}", listing.message());
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

    /// The arguments of a successfully parsed `lock` invocation.
    #[derive(Debug)]
    struct LockArgs {
        flake: String,
        file: Option<PathBuf>,
        check: bool,
        list: bool,
        update: bool,
        images: Vec<String>,
    }

    fn lock_args(args: &[&str]) -> LockArgs {
        match parse(args).unwrap().command {
            Command::Lock {
                flake,
                file,
                check,
                list,
                update,
                images,
            } => LockArgs {
                flake,
                file,
                check,
                list,
                update,
                images,
            },
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn lock_defaults() {
        let a = lock_args(&["lock"]);
        assert_eq!(a.flake, ".");
        assert_eq!(a.file, None);
        assert!(!a.check && !a.list && !a.update && a.images.is_empty());
    }

    #[test]
    fn lock_with_flake_and_file() {
        let a = lock_args(&["lock", "--flake", "/etc/nixos#host", "--file", "x.lock"]);
        assert_eq!(a.flake, "/etc/nixos#host");
        assert_eq!(a.file, Some(PathBuf::from("x.lock")));
        assert!(!a.check);
    }

    #[test]
    fn lock_check_with_flake_and_file() {
        let a = lock_args(&["lock", "--check", "--flake", ".#box", "--file", "x.lock"]);
        assert_eq!(a.flake, ".#box");
        assert_eq!(a.file, Some(PathBuf::from("x.lock")));
        assert!(a.check);
    }

    #[test]
    fn update_takes_optional_canonical_keys() {
        let a = lock_args(&["lock", "--update"]);
        assert!(a.update && a.images.is_empty());

        let a = lock_args(&["lock", "--update", "docker.io/library/caddy:2-alpine", "b"]);
        assert!(a.update);
        assert_eq!(a.images, ["docker.io/library/caddy:2-alpine", "b"]);
    }

    #[test]
    fn list_is_a_mode_of_its_own() {
        let a = lock_args(&["lock", "--list", "--flake", ".#box", "--file", "x.lock"]);
        assert!(a.list && !a.check && !a.update);
        assert_eq!(a.flake, ".#box");
        assert_eq!(a.file, Some(PathBuf::from("x.lock")));

        assert!(parse(&["lock", "--list", "--check"]).is_err());
        assert!(parse(&["lock", "--list", "--update"]).is_err());
        assert!(parse(&["lock", "--list", "docker.io/library/caddy:2-alpine"]).is_err());
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

    fn script(shell: Shell) -> String {
        let mut out = Vec::new();
        completions(shell, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn completions_cover_every_mode_for_each_shell() {
        for shell in [Shell::Bash, Shell::Zsh, Shell::Fish] {
            let script = script(shell);
            for word in ["pin-oci", "lock", "completions"] {
                assert!(script.contains(word), "{shell}: missing '{word}'");
            }
            for flag in ["check", "update", "list", "flake", "file"] {
                // fish declares long flags as `-l <name>`, bash and zsh spell them out.
                let spelled = if shell == Shell::Fish {
                    format!("-l {flag}")
                } else {
                    format!("--{flag}")
                };
                assert!(script.contains(&spelled), "{shell}: missing '{spelled}'");
            }
        }
    }

    #[test]
    fn completions_take_a_known_shell() {
        match parse(&["completions", "zsh"]).unwrap().command {
            Command::Completions { shell } => assert_eq!(shell, Shell::Zsh),
            other => panic!("unexpected command: {other:?}"),
        }
        assert!(parse(&["completions"]).is_err(), "the shell is required");
        assert!(parse(&["completions", "tcsh"]).is_err(), "unknown shell");
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
