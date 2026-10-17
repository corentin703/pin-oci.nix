use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::lockfile::{Lockfile, LockfileError};
use crate::nix::{self, Evaluator, NixError};

const LOCKFILE_NAME: &str = "pin-oci.lock";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Status {
    /// Declared and locked for every platform of the hosts declaring it.
    Locked,
    /// Declared, but with no entry for at least one of those platforms.
    Missing,
    /// Locked, but pulled into the store and lacking the `nixHash` of a platform.
    Unhashed,
    /// In the lockfile, no longer declared.
    Stale,
}

impl Status {
    fn label(&self) -> &'static str {
        match self {
            Self::Locked => "locked",
            Self::Missing => "missing",
            Self::Unhashed => "no-hash",
            Self::Stale => "stale",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    key: String,
    status: Status,
    /// Hosts declaring the image; empty for a stale one.
    hosts: Vec<String>,
    /// `(system, digest)` the lockfile has for the platforms the image is needed on (every
    /// locked platform for a stale image).
    digests: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) struct Listing {
    file: PathBuf,
    systems: String,
    skipped_note: String,
    entries: Vec<Entry>,
}

impl Listing {
    pub(crate) fn message(&self) -> String {
        if self.entries.is_empty() {
            return format!(
                "pin-oci: no image declared or locked for '{}' in {}{}",
                self.systems,
                self.file.display(),
                self.skipped_note
            );
        }
        let hosts: Vec<String> = self
            .entries
            .iter()
            .map(|e| match e.hosts.as_slice() {
                [] => "-".to_owned(),
                hosts => hosts.join(","),
            })
            .collect();
        let key_width = self.entries.iter().map(|e| e.key.len()).max().unwrap_or(0);
        let hosts_width = hosts.iter().map(String::len).max().unwrap_or(0);
        let key_width = key_width.max("IMAGE".len());
        let hosts_width = hosts_width.max("HOSTS".len());
        let row = |status: &str, key: &str, hosts: &str, digest: &str| {
            format!("\n  {status:<7}  {key:<key_width$}  {hosts:<hosts_width$}  {digest}")
                .trim_end()
                .to_owned()
        };
        let mut lines = row("STATUS", "IMAGE", "HOSTS", "DIGEST");
        for (e, hosts) in self.entries.iter().zip(&hosts) {
            let mut digests = e
                .digests
                .iter()
                .map(|(system, digest)| format!("{system} {digest}"));
            lines += &row(
                e.status.label(),
                &e.key,
                hosts,
                &digests.next().unwrap_or_default(),
            );
            for digest in digests {
                lines += &row("", "", "", &digest);
            }
        }
        format!(
            "pin-oci: images for '{}' in {}{lines}{}",
            self.systems,
            self.file.display(),
            self.skipped_note
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
            let image = lock.get(key);
            let Some(systems) = declared.systems.get(key) else {
                let digests = image
                    .into_iter()
                    .flat_map(|image| image.digests())
                    .map(|(system, digest)| (system.to_owned(), digest.to_owned()))
                    .collect();
                return Entry {
                    key: key.to_owned(),
                    status: Status::Stale,
                    hosts: Vec::new(),
                    digests,
                };
            };
            let digests: Vec<(String, String)> = systems
                .iter()
                .filter_map(|system| {
                    let digest = image?.digest(system)?;
                    Some((system.clone(), digest.to_owned()))
                })
                .collect();
            let unhashed = declared.targets[key].pull_image
                && systems.iter().any(|system| {
                    image.is_some_and(|i| i.has_system(system) && i.nix_hash(system).is_none())
                });
            Entry {
                key: key.to_owned(),
                status: if digests.len() != systems.len() {
                    Status::Missing
                } else if unhashed {
                    Status::Unhashed
                } else {
                    Status::Locked
                },
                hosts: declared.hosts[key].iter().cloned().collect(),
                digests,
            }
        })
        .collect();
    Ok(Listing {
        file,
        systems: declared.systems_label(),
        skipped_note: declared.skipped_note(),
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
            listing.entries[1].digests,
            [("x86_64-linux".to_owned(), format!("sha256:{HEX}"))]
        );
        assert!(listing.entries[0].digests.is_empty());
        assert_eq!(listing.entries[1].hosts, ["box"]);
        assert!(listing.entries[2].hosts.is_empty());
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
        assert!(listing.entries[0].digests.is_empty());
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
        let w = CADDY.len();
        assert_eq!(
            lines[1],
            format!("  STATUS   {:<w$}  HOSTS  DIGEST", "IMAGE")
        );
        assert_eq!(lines[2], format!("  missing  {SOLO:<w$}  box"));
        assert_eq!(
            lines[3],
            format!("  locked   {CADDY:<w$}  box    x86_64-linux sha256:{HEX}")
        );
    }

    fn two_hosts() -> StubEvaluator {
        let t = r#"{"registry":"r","repository":"p","tag":"t"}"#;
        StubEvaluator::routed()
            .route(".#nixosConfigurations", r#"["arm","pc","plain"]"#)
            .route(
                ".#nixosConfigurations.\"arm\".config",
                &format!(r#"{{"system":"aarch64-linux","targets":{{"{CADDY}":{t}}}}}"#),
            )
            .route(
                ".#nixosConfigurations.\"pc\".config",
                &format!(r#"{{"system":"x86_64-linux","targets":{{"{CADDY}":{t},"{SOLO}":{t}}}}}"#),
            )
            .route(".#nixosConfigurations.\"plain\".config", "null")
    }

    #[test]
    fn without_a_host_lists_the_union_with_its_hosts_and_every_platform() {
        let file = temp_dir("list-union").join(LOCKFILE_NAME);
        std::fs::write(
            &file,
            lock_json(&[
                (CADDY, "x86_64-linux", HEX),
                (SOLO, "x86_64-linux", OTHER),
                (OLD, "aarch64-linux", OTHER),
            ]),
        )
        .unwrap();

        let listing = list(&two_hosts(), ".", Some(&file)).unwrap();
        // caddy is declared on both platforms but locked on x86_64 only.
        assert_eq!(
            statuses(&listing),
            [
                (SOLO, &Status::Locked),
                (CADDY, &Status::Missing),
                (OLD, &Status::Stale),
            ]
        );
        assert_eq!(listing.entries[0].hosts, ["pc"]);
        assert_eq!(listing.entries[1].hosts, ["arm", "pc"]);
        assert_eq!(
            listing.entries[1].digests,
            [("x86_64-linux".to_owned(), format!("sha256:{HEX}"))]
        );
        assert_eq!(
            listing.entries[2].digests,
            [("aarch64-linux".to_owned(), format!("sha256:{OTHER}"))]
        );

        let message = listing.message();
        assert!(message.contains("images for 'aarch64-linux, x86_64-linux' in "));
        assert!(message.contains("HOSTS"));
        assert!(message.contains("arm,pc"));
        assert!(message.ends_with("\npin-oci: ignored, pin-oci not enabled: plain"));
    }

    #[test]
    fn a_stale_image_keeps_one_line_per_locked_platform() {
        let dir = temp_dir("list-stale-multi");
        std::fs::write(
            dir.join(LOCKFILE_NAME),
            lock_json(&[(OLD, "x86_64-linux", HEX)]).replace(
                r#""arch":{"#,
                &format!(
                    r#""arch":{{"aarch64-linux":{{"digest":"sha256:{OTHER}","os":"linux","architecture":"arm64"}},"#
                ),
            ),
        )
        .unwrap();
        let ev = StubEvaluator::ok(&declared_json(&[]));

        let message = list(&ev, &flake(&dir), None).unwrap().message();
        assert!(message.contains(&format!("aarch64-linux sha256:{OTHER}\n")));
        assert!(message
            .trim_end()
            .ends_with(&format!("x86_64-linux sha256:{HEX}")));
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

    #[test]
    fn an_image_to_pull_without_nix_hash_is_flagged_until_it_has_one() {
        let dir = temp_dir("list-unhashed");
        let declared = format!(
            r#"{{"system":"x86_64-linux","targets":{{"{CADDY}":{{"registry":"r","repository":"p","tag":"t","pullImage":true}}}}}}"#
        );
        let ev = StubEvaluator::ok(&declared);

        std::fs::write(
            dir.join(LOCKFILE_NAME),
            lock_json(&[(CADDY, "x86_64-linux", HEX)]),
        )
        .unwrap();
        let listing = list(&ev, &flake(&dir), None).unwrap();
        assert_eq!(statuses(&listing), [(CADDY, &Status::Unhashed)]);
        assert!(listing.message().contains("no-hash"));

        let hashed = lock_json(&[(CADDY, "x86_64-linux", HEX)]).replace(
            r#""architecture":"amd64""#,
            r#""architecture":"amd64","nixHash":"sha256-AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=""#,
        );
        std::fs::write(dir.join(LOCKFILE_NAME), hashed).unwrap();
        let listing = list(&ev, &flake(&dir), None).unwrap();
        assert_eq!(statuses(&listing), [(CADDY, &Status::Locked)]);
    }
}
