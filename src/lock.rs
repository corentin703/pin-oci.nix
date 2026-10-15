use std::fmt;
use std::path::{Path, PathBuf};

use crate::lockfile::{Lockfile, LockfileError};
use crate::nix::{self, Evaluator, NixError};
use crate::registry::{self, RegistryError, Runtime};

const LOCKFILE_NAME: &str = "pin-oci.lock";

#[derive(Debug)]
pub(crate) struct Outcome {
    file: PathBuf,
    system: String,
    total: usize,
    resolved: Vec<String>,
    pruned: Vec<String>,
    written: bool,
}

impl Outcome {
    pub(crate) fn message(&self) -> String {
        if !self.written {
            return format!(
                "pin-oci: {} already up to date for '{}' ({} image(s))",
                self.file.display(),
                self.system,
                self.total
            );
        }
        let list = |label: &str, keys: &[String]| -> String {
            keys.iter().map(|k| format!("\n  {label} {k}")).collect()
        };
        format!(
            "pin-oci: wrote {} ({} image(s)){}{}",
            self.file.display(),
            self.total,
            list("+", &self.resolved),
            list("-", &self.pruned)
        )
    }
}

#[derive(Debug)]
pub(crate) enum LockError {
    Nix(NixError),
    Lockfile(LockfileError),
    NoLockfilePath(String),
    /// Every image that failed to resolve, in key order. Nothing was written.
    Resolution(Vec<(String, RegistryError)>),
}

impl fmt::Display for LockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nix(e) => e.fmt(f),
            Self::Lockfile(e) => e.fmt(f),
            Self::NoLockfilePath(flake) => {
                write!(f, "'{flake}' is not a local flake: pass --file <path>")
            }
            Self::Resolution(errors) => {
                write!(
                    f,
                    "cannot resolve {} image(s), lockfile unchanged:",
                    errors.len()
                )?;
                for (key, e) in errors {
                    write!(f, "\n  - {key}: {e}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for LockError {}

/// Resolves the images missing from the lockfile and prunes the ones no longer declared.
/// Existing entries are never re-resolved. All or nothing: any failure leaves the file untouched.
pub(crate) fn lock(
    evaluator: &dyn Evaluator,
    runtime: &dyn Runtime,
    flake: &str,
    file: Option<&Path>,
) -> Result<Outcome, LockError> {
    let file = match file {
        Some(file) => file.to_path_buf(),
        None => nix::flake_root(flake)
            .ok_or_else(|| LockError::NoLockfilePath(flake.to_owned()))?
            .join(LOCKFILE_NAME),
    };
    let declared = nix::declared(evaluator, flake).map_err(LockError::Nix)?;
    let mut lockfile = Lockfile::read(&file).map_err(LockError::Lockfile)?;

    let to_resolve = lockfile.missing(
        declared.targets.keys().map(String::as_str),
        &declared.system,
    );
    let mut resolved = Vec::new();
    let mut errors = Vec::new();
    for key in to_resolve {
        let target = &declared.targets[&key];
        let result = registry::resolve(runtime, target).and_then(|image| {
            if image.has_system(&declared.system) {
                Ok(image)
            } else {
                Err(RegistryError::MissingHostPlatform(declared.system.clone()))
            }
        });
        match result {
            Ok(image) => resolved.push((key, image)),
            Err(e) => errors.push((key, e)),
        }
    }
    if !errors.is_empty() {
        return Err(LockError::Resolution(errors));
    }

    let resolved_keys = resolved.iter().map(|(key, _)| key.clone()).collect();
    for (key, image) in resolved {
        lockfile.insert(key, image);
    }
    let pruned = lockfile.prune(|key| declared.targets.contains_key(key));
    let written = lockfile.write(&file).map_err(LockError::Lockfile)?;

    Ok(Outcome {
        file,
        system: declared.system,
        total: declared.targets.len(),
        resolved: resolved_keys,
        pruned,
        written,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{temp_dir, StubEvaluator, StubRuntime};

    const INDEX: &str = include_str!("../tests/fixtures/skopeo-index.json");
    const SINGLE_RAW: &str = include_str!("../tests/fixtures/skopeo-single-raw.json");
    const SINGLE_INSPECT: &str = include_str!("../tests/fixtures/skopeo-single-inspect.json");
    const CADDY: &str = "docker.io/library/caddy:2-alpine";
    const SOLO: &str = "docker.io/acme/solo:1";
    const CADDY_REF: &str = "docker://docker.io/library/caddy:2-alpine";
    const SOLO_REF: &str = "docker://docker.io/acme/solo:1";

    fn declared(system: &str, keys: &[&str]) -> String {
        let targets: Vec<String> = keys
            .iter()
            .map(|k| {
                let (registry, rest) = k.split_once('/').unwrap();
                let (repository, tag) = rest.rsplit_once(':').unwrap();
                format!(
                    r#""{k}":{{"registry":"{registry}","repository":"{repository}","tag":"{tag}"}}"#
                )
            })
            .collect();
        format!(
            r#"{{"system":"{system}","targets":{{{}}}}}"#,
            targets.join(",")
        )
    }

    fn runtime() -> StubRuntime {
        StubRuntime::new()
            .on(&["inspect", "--raw", CADDY_REF], INDEX)
            .on(&["inspect", "--raw", SOLO_REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", SOLO_REF], SINGLE_INSPECT)
            .failing(
                &["inspect", "--raw", "docker://docker.io/library/nope:1"],
                "manifest unknown",
            )
    }

    fn flake(dir: &Path) -> String {
        format!("{}#box", dir.display())
    }

    #[test]
    fn creates_the_lockfile_then_is_idempotent_without_calling_skopeo() {
        let dir = temp_dir("lock-create");
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));
        let rt = runtime();

        let first = lock(&ev, &rt, &flake(&dir), None).unwrap();
        assert!(first.written && first.resolved == vec![SOLO, CADDY]);
        let content = std::fs::read_to_string(dir.join(LOCKFILE_NAME)).unwrap();
        let calls = rt.calls.borrow().len();
        assert_eq!(calls, 3);

        let second = lock(&ev, &rt, &flake(&dir), None).unwrap();
        assert!(!second.written && second.resolved.is_empty());
        assert_eq!(
            rt.calls.borrow().len(),
            calls,
            "no re-resolution of locked images"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join(LOCKFILE_NAME)).unwrap(),
            content
        );
    }

    #[test]
    fn output_does_not_depend_on_target_order() {
        let (a, b) = (temp_dir("lock-order-a"), temp_dir("lock-order-b"));
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO])),
            &runtime(),
            &flake(&a),
            None,
        )
        .unwrap();
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[SOLO, CADDY])),
            &runtime(),
            &flake(&b),
            None,
        )
        .unwrap();
        assert_eq!(
            std::fs::read(a.join(LOCKFILE_NAME)).unwrap(),
            std::fs::read(b.join(LOCKFILE_NAME)).unwrap()
        );
    }

    #[test]
    fn existing_entries_are_kept_new_ones_added_and_stale_ones_pruned() {
        let dir = temp_dir("lock-merge");
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO])),
            &runtime(),
            &flake(&dir),
            None,
        )
        .unwrap();

        // caddy stays, solo is dropped from the configuration.
        let rt = runtime();
        let out = lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[CADDY])),
            &rt,
            &flake(&dir),
            None,
        )
        .unwrap();
        assert!(out.written && out.resolved.is_empty() && out.pruned == vec![SOLO]);
        assert!(rt.calls.borrow().is_empty());
        let content = std::fs::read_to_string(dir.join(LOCKFILE_NAME)).unwrap();
        assert!(content.contains(CADDY) && !content.contains(SOLO));
    }

    #[test]
    fn host_platform_missing_from_a_locked_entry_triggers_resolution() {
        let dir = temp_dir("lock-new-arch");
        lock(
            &StubEvaluator::ok(&declared("aarch64-linux", &[SOLO])),
            &runtime(),
            &flake(&dir),
            None,
        )
        .expect_err("solo has no arm64 manifest");
        // Locked for x86_64 only; evaluating on riscv64 must not be considered in sync.
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[SOLO])),
            &runtime(),
            &flake(&dir),
            None,
        )
        .unwrap();
        let rt = runtime();
        let err = lock(
            &StubEvaluator::ok(&declared("riscv64-linux", &[SOLO])),
            &rt,
            &flake(&dir),
            None,
        )
        .unwrap_err();
        assert!(matches!(err, LockError::Resolution(_)));
        assert!(!rt.calls.borrow().is_empty());
    }

    #[test]
    fn all_or_nothing_on_resolution_failure() {
        let dir = temp_dir("lock-atomic");
        let path = dir.join(LOCKFILE_NAME);
        let nope = "docker.io/library/nope:1";

        // Nothing is created when the first run fails...
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, nope]));
        let err = lock(&ev, &runtime(), &flake(&dir), None).unwrap_err();
        assert!(!path.exists());
        let msg = err.to_string();
        assert!(
            msg.contains(nope)
                && msg.contains("manifest unknown")
                && !msg.contains(&format!("- {CADDY}"))
        );

        // ...and an existing lockfile stays byte-identical.
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[SOLO])),
            &runtime(),
            &flake(&dir),
            None,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        lock(&ev, &runtime(), &flake(&dir), None).unwrap_err();
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn every_failure_is_reported() {
        let dir = temp_dir("lock-all-errors");
        let ev = StubEvaluator::ok(&declared(
            "x86_64-linux",
            &["docker.io/library/nope:1", "docker.io/library/nope:2"],
        ));
        let rt = runtime().failing(
            &["inspect", "--raw", "docker://docker.io/library/nope:2"],
            "denied",
        );
        let LockError::Resolution(errors) = lock(&ev, &rt, &flake(&dir), None).unwrap_err() else {
            panic!("expected a resolution error");
        };
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn nix_failure_corrupt_lockfile_and_remote_flake_are_errors() {
        let dir = temp_dir("lock-errors");
        let rt = StubRuntime::new();

        let ev = StubEvaluator::failing("boom");
        assert!(matches!(
            lock(&ev, &rt, &flake(&dir), None),
            Err(LockError::Nix(_))
        ));

        std::fs::write(dir.join(LOCKFILE_NAME), "{").unwrap();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY]));
        assert!(matches!(
            lock(&ev, &rt, &flake(&dir), None),
            Err(LockError::Lockfile(_))
        ));

        assert!(matches!(
            lock(&ev, &rt, "github:owner/repo#box", None),
            Err(LockError::NoLockfilePath(_))
        ));
        assert!(rt.calls.borrow().is_empty());
    }
}
