use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::lockfile::{Lockfile, LockfileError};
use crate::nix::{self, Evaluator, NixError};

const LOCKFILE_NAME: &str = "pin-oci.lock";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Declared and locked for the host platform.
    Locked,
    /// Declared, but with no entry for the host platform.
    Missing,
    /// In the lockfile, no longer declared.
    Stale,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Missing => "missing",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    key: String,
    status: Status,
    /// Digest for the host platform, when the lockfile has one.
    digest: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Listing {
    file: PathBuf,
    system: String,
    entries: Vec<Entry>,
}

impl Listing {
    pub(crate) fn message(&self) -> String {
        if self.entries.is_empty() {
            return format!(
                "pin-oci: no image declared or locked for '{}' in {}",
                self.system,
                self.file.display()
            );
        }
        let width = self.entries.iter().map(|e| e.key.len()).max().unwrap_or(0);
        let lines: String = self
            .entries
            .iter()
            .map(|e| {
                let digest = e.digest.as_deref().unwrap_or("");
                format!("\n  {:<7}  {:<width$}  {digest}", e.status.label(), e.key)
                    .trim_end()
                    .to_owned()
            })
            .collect();
        format!(
            "pin-oci: images for '{}' in {}{lines}",
            self.system,
            self.file.display()
        )
    }
}

#[derive(Debug)]
pub(crate) enum ListError {
    Nix(NixError),
    Lockfile(LockfileError),
    NoLockfilePath(String),
}

impl fmt::Display for ListError {
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

impl std::error::Error for ListError {}

/// Every declared and every locked image, with its status for the host platform. Read-only:
/// it never writes and, taking no `Runtime`, cannot contact a registry.
pub(crate) fn list(
    evaluator: &dyn Evaluator,
    flake: &str,
    file: Option<&Path>,
) -> Result<Listing, ListError> {
    let file = match file {
        Some(file) => file.to_path_buf(),
        None => nix::flake_root(flake)
            .ok_or_else(|| ListError::NoLockfilePath(flake.to_owned()))?
            .join(LOCKFILE_NAME),
    };
    let declared = nix::declared(evaluator, flake).map_err(ListError::Nix)?;
    let lock = Lockfile::read(&file).map_err(ListError::Lockfile)?;

    let keys: BTreeSet<&str> = declared
        .targets
        .keys()
        .map(String::as_str)
        .chain(lock.keys())
        .collect();
    let entries = keys
        .into_iter()
        .map(|key| {
            let digest = lock
                .get(key)
                .and_then(|image| image.digest(&declared.system))
                .map(str::to_owned);
            let status = match (declared.targets.contains_key(key), &digest) {
                (false, _) => Status::Stale,
                (true, Some(_)) => Status::Locked,
                (true, None) => Status::Missing,
            };
            Entry {
                key: key.to_owned(),
                status,
                digest,
            }
        })
        .collect();
    Ok(Listing {
        file,
        system: declared.system,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{temp_dir, StubEvaluator};

    const CADDY: &str = "docker.io/library/caddy:2-alpine";
    const SOLO: &str = "docker.io/acme/solo:1";
    const OLD: &str = "docker.io/old/thing:3";
    const HEX: &str = "d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499abcd";
    const OTHER: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    /// `(key, system, digest hex)` entries of a lockfile.
    fn lock_json(entries: &[(&str, &str, &str)]) -> String {
        let images: Vec<String> = entries
            .iter()
            .map(|(key, system, hex)| {
                format!(
                    r#""{key}":{{"registry":"r","repository":"p","tag":"t","arch":{{"{system}":{{"digest":"sha256:{hex}","os":"linux","architecture":"amd64"}}}}}}"#
                )
            })
            .collect();
        format!(r#"{{"version":1,"images":{{{}}}}}"#, images.join(","))
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

    fn flake(dir: &Path) -> String {
        format!("{}#box", dir.display())
    }

    fn statuses(listing: &Listing) -> Vec<(&str, &Status)> {
        listing
            .entries
            .iter()
            .map(|e| (e.key.as_str(), &e.status))
            .collect()
    }

    #[test]
    fn classifies_locked_missing_and_stale_images() {
        let dir = temp_dir("list-status");
        std::fs::write(
            dir.join(LOCKFILE_NAME),
            lock_json(&[(CADDY, "x86_64-linux", HEX), (OLD, "x86_64-linux", OTHER)]),
        )
        .unwrap();
        let ev = StubEvaluator::ok(&declared_json(&[CADDY, SOLO]));

        let listing = list(&ev, &flake(&dir), None).unwrap();
        assert_eq!(
            statuses(&listing),
            [
                (SOLO, &Status::Missing),
                (CADDY, &Status::Locked),
                (OLD, &Status::Stale),
            ]
        );
        assert_eq!(
            listing.entries[1].digest.as_deref(),
            Some(&*format!("sha256:{HEX}"))
        );
        assert_eq!(listing.entries[0].digest, None);
    }

    #[test]
    fn only_the_host_platform_counts() {
        let dir = temp_dir("list-arch");
        // Locked for aarch64 only: on x86_64 the image is missing, with no digest to show.
        std::fs::write(
            dir.join(LOCKFILE_NAME),
            lock_json(&[(CADDY, "aarch64-linux", HEX)]),
        )
        .unwrap();
        let ev = StubEvaluator::ok(&declared_json(&[CADDY]));

        let listing = list(&ev, &flake(&dir), None).unwrap();
        assert_eq!(statuses(&listing), [(CADDY, &Status::Missing)]);
        assert_eq!(listing.entries[0].digest, None);
    }

    #[test]
    fn message_is_aligned_sorted_and_shows_digests() {
        let dir = temp_dir("list-message");
        std::fs::write(
            dir.join(LOCKFILE_NAME),
            lock_json(&[(CADDY, "x86_64-linux", HEX)]),
        )
        .unwrap();
        let ev = StubEvaluator::ok(&declared_json(&[CADDY, SOLO]));

        let message = list(&ev, &flake(&dir), None).unwrap().message();
        let lines: Vec<&str> = message.lines().collect();
        assert!(lines[0].starts_with("pin-oci: images for 'x86_64-linux' in "));
        assert_eq!(lines[1], format!("  missing  {SOLO}"));
        assert_eq!(
            lines[2],
            format!(
                "  locked   {CADDY:<width$}  sha256:{HEX}",
                width = CADDY.len()
            )
        );
    }

    #[test]
    fn nothing_declared_or_locked_says_so() {
        let dir = temp_dir("list-empty");
        let ev = StubEvaluator::ok(&declared_json(&[]));

        let listing = list(&ev, &flake(&dir), None).unwrap();
        assert!(listing.entries.is_empty());
        assert!(listing.message().contains("no image declared or locked"));
    }

    #[test]
    fn never_writes_the_lockfile() {
        let dir = temp_dir("list-readonly");
        let ev = StubEvaluator::ok(&declared_json(&[CADDY]));

        list(&ev, &flake(&dir), None).unwrap();
        assert!(
            !dir.join(LOCKFILE_NAME).exists(),
            "an absent lockfile stays absent"
        );

        let content = lock_json(&[(OLD, "x86_64-linux", OTHER)]);
        std::fs::write(dir.join(LOCKFILE_NAME), &content).unwrap();
        list(&ev, &flake(&dir), None).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join(LOCKFILE_NAME)).unwrap(),
            content
        );
    }

    #[test]
    fn explicit_file_overrides_flake_root() {
        let dir = temp_dir("list-explicit-file");
        let file = dir.join("custom.lock");
        std::fs::write(&file, lock_json(&[(CADDY, "x86_64-linux", HEX)])).unwrap();
        let ev = StubEvaluator::ok(&declared_json(&[CADDY]));

        let listing = list(&ev, "github:owner/repo#box", Some(&file)).unwrap();
        assert_eq!(statuses(&listing), [(CADDY, &Status::Locked)]);
    }

    #[test]
    fn remote_flake_without_file_nix_failure_and_corrupt_lockfile_are_errors() {
        let ev = StubEvaluator::ok(&declared_json(&[CADDY]));
        let err = list(&ev, "github:owner/repo#box", None).unwrap_err();
        assert!(matches!(err, ListError::NoLockfilePath(_)));
        assert!(ev.calls.borrow().is_empty());

        let dir = temp_dir("list-errors");
        let failing = StubEvaluator::failing("boom");
        assert!(matches!(
            list(&failing, &flake(&dir), None),
            Err(ListError::Nix(_))
        ));

        std::fs::write(dir.join(LOCKFILE_NAME), "{").unwrap();
        assert!(matches!(
            list(&ev, &flake(&dir), None),
            Err(ListError::Lockfile(_))
        ));
    }
}
