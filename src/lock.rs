use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use crate::lockfile::{Image, Lockfile, LockfileError};
use crate::nix::{self, Evaluator, NixError};
use crate::registry::{self, RegistryError, Runtime};

const LOCKFILE_NAME: &str = "pin-oci.lock";

/// Which already-locked images are re-resolved. Missing ones are always resolved.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Refresh {
    Missing,
    All,
    /// Canonical keys, exactly as they appear in the lockfile.
    Only(Vec<String>),
}

/// How a platform digest moved when an image was refreshed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Change {
    system: String,
    old: Option<String>,
    new: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Outcome {
    file: PathBuf,
    system: String,
    total: usize,
    resolved: Vec<String>,
    /// Refreshed images whose digests moved, with the platforms concerned.
    updated: Vec<(String, Vec<Change>)>,
    /// Refreshed images that already pointed at the latest digests.
    current: usize,
    pruned: Vec<String>,
    written: bool,
}

fn short(digest: Option<&str>) -> String {
    match digest {
        Some(d) => format!("{}…", &d[..d.len().min("sha256:".len() + 12)]),
        None => "(none)".to_owned(),
    }
}

/// Platforms whose digest differs between the locked entry and the freshly resolved one.
fn changes(old: &Image, new: &Image) -> Vec<Change> {
    let old: BTreeMap<&str, &str> = old.digests().collect();
    let new: BTreeMap<&str, &str> = new.digests().collect();
    let systems: BTreeSet<&str> = old.keys().chain(new.keys()).copied().collect();
    systems
        .into_iter()
        .filter(|system| old.get(system) != new.get(system))
        .map(|system| Change {
            system: system.to_owned(),
            old: old.get(system).map(|d| (*d).to_owned()),
            new: new.get(system).map(|d| (*d).to_owned()),
        })
        .collect()
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
        let updated: String = self
            .updated
            .iter()
            .map(|(key, changes)| {
                let lines: String = changes
                    .iter()
                    .map(|c| {
                        format!(
                            "\n      {} {} -> {}",
                            c.system,
                            short(c.old.as_deref()),
                            short(c.new.as_deref())
                        )
                    })
                    .collect();
                format!("\n  ~ {key}{lines}")
            })
            .collect();
        let current = match self.current {
            0 => String::new(),
            n => format!("\n  = {n} image(s) already current"),
        };
        format!(
            "pin-oci: wrote {} ({} image(s)){}{}{}{}",
            self.file.display(),
            self.total,
            list("+", &self.resolved),
            updated,
            list("-", &self.pruned),
            current
        )
    }
}

#[derive(Debug)]
pub(crate) enum LockError {
    Nix(NixError),
    Lockfile(LockfileError),
    NoLockfilePath(String),
    /// Keys given to `--update` that no declared image has. Nothing was resolved.
    UnknownImages {
        unknown: Vec<String>,
        declared: Vec<String>,
    },
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
            Self::UnknownImages { unknown, declared } => {
                write!(f, "not a declared image: {}", unknown.join(", "))?;
                if declared.is_empty() {
                    write!(f, "\nno image is declared")?;
                } else {
                    write!(f, "\ndeclared images:")?;
                    for key in declared {
                        write!(f, "\n  - {key}")?;
                    }
                }
                Ok(())
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
/// Locked entries are only re-resolved when `refresh` selects them. All or nothing: any
/// failure leaves the file untouched.
pub(crate) fn lock(
    evaluator: &dyn Evaluator,
    runtime: &dyn Runtime,
    flake: &str,
    file: Option<&Path>,
    refresh: &Refresh,
) -> Result<Outcome, LockError> {
    let file = match file {
        Some(file) => file.to_path_buf(),
        None => nix::flake_root(flake)
            .ok_or_else(|| LockError::NoLockfilePath(flake.to_owned()))?
            .join(LOCKFILE_NAME),
    };
    let declared = nix::declared(evaluator, flake).map_err(LockError::Nix)?;
    let mut lockfile = Lockfile::read(&file).map_err(LockError::Lockfile)?;

    let missing = lockfile.missing(
        declared.targets.keys().map(String::as_str),
        &declared.system,
    );
    let selected: BTreeSet<String> = match refresh {
        Refresh::Missing => BTreeSet::new(),
        Refresh::All => declared.targets.keys().cloned().collect(),
        Refresh::Only(keys) => {
            let unknown: Vec<String> = keys
                .iter()
                .filter(|key| !declared.targets.contains_key(*key))
                .cloned()
                .collect();
            if !unknown.is_empty() {
                return Err(LockError::UnknownImages {
                    unknown,
                    declared: declared.targets.keys().cloned().collect(),
                });
            }
            keys.iter().cloned().collect()
        }
    };
    let to_resolve: BTreeSet<String> = missing.iter().cloned().chain(selected).collect();
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

    let mut resolved_keys = Vec::new();
    let mut updated = Vec::new();
    let mut current = 0;
    for (key, image) in resolved {
        if missing.contains(&key) {
            resolved_keys.push(key.clone());
        } else {
            // Selected for a refresh and already locked for this platform.
            let moved = lockfile
                .get(&key)
                .map(|old| changes(old, &image))
                .unwrap_or_default();
            if moved.is_empty() {
                current += 1;
            } else {
                updated.push((key.clone(), moved));
            }
        }
        lockfile.insert(key, image);
    }
    let pruned = lockfile.prune(|key| declared.targets.contains_key(key));
    let written = lockfile.write(&file).map_err(LockError::Lockfile)?;

    Ok(Outcome {
        file,
        system: declared.system,
        total: declared.targets.len(),
        resolved: resolved_keys,
        updated,
        current,
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

        let first = lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing).unwrap();
        assert!(first.written && first.resolved == vec![SOLO, CADDY]);
        let content = std::fs::read_to_string(dir.join(LOCKFILE_NAME)).unwrap();
        let calls = rt.calls.borrow().len();
        assert_eq!(calls, 3);

        let second = lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing).unwrap();
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
            &Refresh::Missing,
        )
        .unwrap();
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[SOLO, CADDY])),
            &runtime(),
            &flake(&b),
            None,
            &Refresh::Missing,
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
            &Refresh::Missing,
        )
        .unwrap();

        // caddy stays, solo is dropped from the configuration.
        let rt = runtime();
        let out = lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[CADDY])),
            &rt,
            &flake(&dir),
            None,
            &Refresh::Missing,
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
            &Refresh::Missing,
        )
        .expect_err("solo has no arm64 manifest");
        // Locked for x86_64 only; evaluating on riscv64 must not be considered in sync.
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[SOLO])),
            &runtime(),
            &flake(&dir),
            None,
            &Refresh::Missing,
        )
        .unwrap();
        let rt = runtime();
        let err = lock(
            &StubEvaluator::ok(&declared("riscv64-linux", &[SOLO])),
            &rt,
            &flake(&dir),
            None,
            &Refresh::Missing,
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
        let err = lock(&ev, &runtime(), &flake(&dir), None, &Refresh::Missing).unwrap_err();
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
            &Refresh::Missing,
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        lock(&ev, &runtime(), &flake(&dir), None, &Refresh::Missing).unwrap_err();
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
        let LockError::Resolution(errors) =
            lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing).unwrap_err()
        else {
            panic!("expected a resolution error");
        };
        assert_eq!(errors.len(), 2);
    }

    const AMD64: &str = "sha256:173b26306d711395accaeba8b67afcaad2a085ccb1e6bf26010bfe8c095a5229";
    const ARM64: &str = "sha256:84058f1a0e5beb97664a9b79dcfd267b594c033d5bb88d8463cdfb59c0779197";
    const NEW_AMD64: &str =
        "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    const NEW_ARM64: &str =
        "sha256:ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

    /// The registry after the tag moved: caddy's amd64 and arm64 manifests have new digests.
    fn moved_runtime() -> StubRuntime {
        let moved = INDEX.replace(AMD64, NEW_AMD64).replace(ARM64, NEW_ARM64);
        StubRuntime::new()
            .on(&["inspect", "--raw", CADDY_REF], &moved)
            .on(&["inspect", "--raw", SOLO_REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", SOLO_REF], SINGLE_INSPECT)
    }

    fn locked(dir: &Path) -> Lockfile {
        Lockfile::read(&dir.join(LOCKFILE_NAME)).unwrap()
    }

    fn digest(lockfile: &Lockfile, key: &str, system: &str) -> String {
        lockfile
            .get(key)
            .and_then(|image| image.platform(system))
            .map(|(digest, _, _)| digest.to_owned())
            .unwrap_or_else(|| panic!("no {system} digest for {key}"))
    }

    fn locked_dir(name: &str) -> PathBuf {
        let dir = temp_dir(name);
        lock(
            &StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO])),
            &runtime(),
            &flake(&dir),
            None,
            &Refresh::Missing,
        )
        .unwrap();
        dir
    }

    #[test]
    fn update_all_moves_every_platform_of_a_changed_image_and_reports_it() {
        let dir = locked_dir("update-all");
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        let out = lock(&ev, &moved_runtime(), &flake(&dir), None, &Refresh::All).unwrap();
        assert!(out.written && out.resolved.is_empty() && out.pruned.is_empty());
        assert_eq!(out.current, 1, "solo did not move");
        assert_eq!(out.updated.len(), 1);
        let (key, changes) = &out.updated[0];
        assert_eq!(key, CADDY);
        let systems: Vec<&str> = changes.iter().map(|c| c.system.as_str()).collect();
        assert_eq!(
            systems,
            ["aarch64-linux", "x86_64-linux"],
            "only what moved"
        );
        assert_eq!(changes[1].old.as_deref(), Some(AMD64));
        assert_eq!(changes[1].new.as_deref(), Some(NEW_AMD64));

        let after = locked(&dir);
        assert_eq!(digest(&after, CADDY, "x86_64-linux"), NEW_AMD64);
        assert_eq!(digest(&after, CADDY, "aarch64-linux"), NEW_ARM64);
        let message = out.message();
        assert!(message.contains(&format!("~ {CADDY}")), "{message}");
        assert!(message.contains("x86_64-linux sha256:173b26306d71… -> sha256:eeeeeeeeeeee…"));
        assert!(
            message.contains("= 1 image(s) already current"),
            "{message}"
        );
    }

    #[test]
    fn update_only_touches_the_selected_images() {
        let dir = locked_dir("update-only");
        let rt = moved_runtime();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        let out = lock(
            &ev,
            &rt,
            &flake(&dir),
            None,
            &Refresh::Only(vec![CADDY.to_owned()]),
        )
        .unwrap();
        assert_eq!(out.updated.len(), 1);
        assert_eq!(
            *rt.calls.borrow(),
            [format!("inspect --raw {CADDY_REF}")],
            "solo is not even looked up"
        );
        assert_eq!(digest(&locked(&dir), CADDY, "x86_64-linux"), NEW_AMD64);
    }

    #[test]
    fn update_when_nothing_moved_changes_nothing() {
        let dir = locked_dir("update-same");
        let path = dir.join(LOCKFILE_NAME);
        let before = std::fs::read(&path).unwrap();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        let out = lock(&ev, &runtime(), &flake(&dir), None, &Refresh::All).unwrap();
        assert!(!out.written && out.updated.is_empty());
        assert!(out.message().contains("already up to date"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn plain_lock_never_moves_a_locked_digest() {
        let dir = locked_dir("update-not-requested");
        let rt = moved_runtime();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        let out = lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing).unwrap();
        assert!(!out.written);
        assert!(rt.calls.borrow().is_empty());
        assert_eq!(digest(&locked(&dir), CADDY, "x86_64-linux"), AMD64);
    }

    #[test]
    fn update_with_an_unknown_key_fails_before_resolving_anything() {
        let dir = locked_dir("update-unknown");
        let path = dir.join(LOCKFILE_NAME);
        let before = std::fs::read(&path).unwrap();
        let rt = moved_runtime();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        // `caddy:2-alpine` is not a canonical key: the CLI never normalizes.
        let err = lock(
            &ev,
            &rt,
            &flake(&dir),
            None,
            &Refresh::Only(vec![CADDY.to_owned(), "caddy:2-alpine".to_owned()]),
        )
        .unwrap_err();
        let LockError::UnknownImages { unknown, declared } = &err else {
            panic!("expected UnknownImages, got {err}");
        };
        assert_eq!(unknown, &["caddy:2-alpine"]);
        assert_eq!(declared, &[SOLO, CADDY]);
        assert!(err.to_string().contains(&format!("  - {CADDY}")));
        assert!(
            rt.calls.borrow().is_empty(),
            "validated before any skopeo call"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn a_failed_update_leaves_the_lockfile_untouched() {
        let dir = locked_dir("update-failure");
        let path = dir.join(LOCKFILE_NAME);
        let before = std::fs::read(&path).unwrap();
        // solo resolves to new content, caddy fails: nothing may be written.
        let rt = StubRuntime::new()
            .failing(&["inspect", "--raw", CADDY_REF], "denied")
            .on(&["inspect", "--raw", SOLO_REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", SOLO_REF], SINGLE_INSPECT);
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY, SOLO]));

        let err = lock(&ev, &rt, &flake(&dir), None, &Refresh::All).unwrap_err();
        assert!(matches!(err, LockError::Resolution(ref e) if e.len() == 1));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn update_still_resolves_missing_and_prunes_stale_entries() {
        let dir = locked_dir("update-merge");
        let rt = moved_runtime();
        // solo is no longer declared; caddy stays.
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY]));

        let out = lock(&ev, &rt, &flake(&dir), None, &Refresh::All).unwrap();
        assert_eq!(out.pruned, vec![SOLO]);
        assert_eq!(out.updated.len(), 1);
        assert!(!std::fs::read_to_string(dir.join(LOCKFILE_NAME))
            .unwrap()
            .contains(SOLO));
    }

    #[test]
    fn nix_failure_corrupt_lockfile_and_remote_flake_are_errors() {
        let dir = temp_dir("lock-errors");
        let rt = StubRuntime::new();

        let ev = StubEvaluator::failing("boom");
        assert!(matches!(
            lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing),
            Err(LockError::Nix(_))
        ));

        std::fs::write(dir.join(LOCKFILE_NAME), "{").unwrap();
        let ev = StubEvaluator::ok(&declared("x86_64-linux", &[CADDY]));
        assert!(matches!(
            lock(&ev, &rt, &flake(&dir), None, &Refresh::Missing),
            Err(LockError::Lockfile(_))
        ));

        assert!(matches!(
            lock(&ev, &rt, "github:owner/repo#box", None, &Refresh::Missing),
            Err(LockError::NoLockfilePath(_))
        ));
        assert!(rt.calls.borrow().is_empty());
    }
}
