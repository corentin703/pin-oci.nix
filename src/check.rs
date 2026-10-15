use std::fmt;
use std::path::{Path, PathBuf};

use crate::lockfile::{Lockfile, LockfileError};
use crate::nix::{self, Evaluator, NixError};

const LOCKFILE_NAME: &str = "pin-oci.lock";

#[derive(Debug)]
pub(crate) struct Report {
    file: PathBuf,
    system: String,
    declared: usize,
    missing: Vec<String>,
}

impl Report {
    pub(crate) fn is_in_sync(&self) -> bool {
        self.missing.is_empty()
    }

    pub(crate) fn message(&self) -> String {
        if self.is_in_sync() {
            return format!(
                "pin-oci: {} image(s) pinned for '{}' in {}",
                self.declared,
                self.system,
                self.file.display()
            );
        }
        let list: String = self.missing.iter().map(|k| format!("\n  - {k}")).collect();
        format!(
            "pin-oci: lockfile out of sync for '{}' ({}):{list}\n\nRun: pin-oci lock",
            self.system,
            self.file.display()
        )
    }
}

#[derive(Debug)]
pub(crate) enum CheckError {
    Nix(NixError),
    Lockfile(LockfileError),
    NoLockfilePath(String),
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nix(e) => e.fmt(f),
            Self::Lockfile(e) => e.fmt(f),
            Self::NoLockfilePath(flake) => {
                write!(f, "'{flake}' is not a local flake: pass --file <path>")
            }
        }
    }
}

impl std::error::Error for CheckError {}

pub(crate) fn check(
    evaluator: &dyn Evaluator,
    flake: &str,
    file: Option<&Path>,
) -> Result<Report, CheckError> {
    let file = match file {
        Some(file) => file.to_path_buf(),
        None => nix::flake_root(flake)
            .ok_or_else(|| CheckError::NoLockfilePath(flake.to_owned()))?
            .join(LOCKFILE_NAME),
    };
    let declared = nix::declared(evaluator, flake).map_err(CheckError::Nix)?;
    let lock = Lockfile::read(&file).map_err(CheckError::Lockfile)?;
    let missing = lock.missing(
        declared.targets.keys().map(String::as_str),
        &declared.system,
    );
    Ok(Report {
        file,
        system: declared.system,
        declared: declared.targets.len(),
        missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{temp_dir, StubEvaluator};

    const HEX: &str = "d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499abcd";
    const KEY: &str = "docker.io/library/caddy:2-alpine";

    fn lock_json(system: &str) -> String {
        format!(
            r#"{{"version":1,"images":{{"{KEY}":{{"registry":"docker.io","repository":"library/caddy",
"tag":"2-alpine","arch":{{"{system}":{{"digest":"sha256:{HEX}","os":"linux","architecture":"amd64"}}}}}}}}}}"#
        )
    }

    fn declared_json(targets: &[&str]) -> String {
        let list: Vec<String> = targets
            .iter()
            .map(|t| format!(r#""{t}":{{"registry":"r","repository":"p","tag":"t"}}"#))
            .collect();
        format!(
            r#"{{"system":"x86_64-linux","targets":{{{}}}}}"#,
            list.join(",")
        )
    }

    #[test]
    fn in_sync_when_every_target_is_pinned_for_the_system() {
        let dir = temp_dir("check-sync");
        std::fs::write(dir.join(LOCKFILE_NAME), lock_json("x86_64-linux")).unwrap();
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));

        let report = check(&stub, &format!("{}#box", dir.display()), None).unwrap();
        assert!(report.is_in_sync());
        assert!(report.message().contains("1 image(s)"));
    }

    #[test]
    fn out_of_sync_lists_missing_targets_and_the_command() {
        let dir = temp_dir("check-out-of-sync");
        std::fs::write(dir.join(LOCKFILE_NAME), lock_json("x86_64-linux")).unwrap();
        let stub = StubEvaluator::ok(&declared_json(&[KEY, "docker.io/library/redis:7"]));

        let report = check(&stub, &format!("{}#box", dir.display()), None).unwrap();
        assert!(!report.is_in_sync());
        let msg = report.message();
        assert!(msg.contains("docker.io/library/redis:7") && !msg.contains("caddy"));
        assert!(msg.contains("pin-oci lock"));
    }

    #[test]
    fn out_of_sync_when_architecture_is_not_locked() {
        let dir = temp_dir("check-wrong-arch");
        std::fs::write(dir.join(LOCKFILE_NAME), lock_json("aarch64-linux")).unwrap();
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));

        let report = check(&stub, &format!("{}#box", dir.display()), None).unwrap();
        assert_eq!(report.missing, vec![KEY]);
    }

    #[test]
    fn absent_lockfile_reports_everything_missing() {
        let dir = temp_dir("check-no-lock");
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));

        let report = check(&stub, &format!("{}#box", dir.display()), None).unwrap();
        assert_eq!(report.missing, vec![KEY]);
    }

    #[test]
    fn explicit_file_overrides_flake_root() {
        let dir = temp_dir("check-explicit-file");
        let file = dir.join("custom.lock");
        std::fs::write(&file, lock_json("x86_64-linux")).unwrap();
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));

        let report = check(&stub, "github:owner/repo#box", Some(&file)).unwrap();
        assert!(report.is_in_sync());
    }

    #[test]
    fn remote_flake_without_file_is_an_error() {
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));
        let err = check(&stub, "github:owner/repo#box", None).unwrap_err();
        assert!(matches!(err, CheckError::NoLockfilePath(_)));
        assert!(stub.calls.borrow().is_empty());
    }

    #[test]
    fn nix_failure_and_corrupt_lockfile_are_errors() {
        let dir = temp_dir("check-errors");
        let flake = format!("{}#box", dir.display());

        let stub = StubEvaluator::failing("boom");
        assert!(matches!(
            check(&stub, &flake, None),
            Err(CheckError::Nix(_))
        ));

        std::fs::write(dir.join(LOCKFILE_NAME), "{").unwrap();
        let stub = StubEvaluator::ok(&declared_json(&[KEY]));
        assert!(matches!(
            check(&stub, &flake, None),
            Err(CheckError::Lockfile(_))
        ));
    }
}
