use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lockfile {
    version: u32,
    images: BTreeMap<String, Image>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Image {
    registry: String,
    repository: String,
    tag: String,
    arch: BTreeMap<String, Platform>,
}

impl Image {
    pub(crate) fn new(
        registry: String,
        repository: String,
        tag: String,
        arch: BTreeMap<String, Platform>,
    ) -> Self {
        Self {
            registry,
            repository,
            tag,
            arch,
        }
    }

    pub(crate) fn has_system(&self, system: &str) -> bool {
        self.arch.contains_key(system)
    }

    pub(crate) fn nix_hash(&self, system: &str) -> Option<&str> {
        self.arch.get(system).and_then(Platform::nix_hash)
    }

    /// What `dockerTools.pullImage` is given for `system`.
    pub(crate) fn pull_spec(&self, system: &str) -> Option<PullSpec> {
        let platform = self.arch.get(system)?;
        Some(PullSpec {
            name: format!("{}/{}", self.registry, self.repository),
            tag: self.tag.clone(),
            digest: platform.digest.clone(),
            os: platform.os.clone(),
            architecture: platform.architecture.clone(),
        })
    }

    /// Records the hash computed for `system`; false when the image has no such platform.
    pub(crate) fn set_nix_hash(&mut self, system: &str, hash: String) -> bool {
        self.arch
            .get_mut(system)
            .map(|p| p.nix_hash = Some(hash))
            .is_some()
    }

    /// Keeps the hashes of `old` for the platforms whose digest did not move.
    pub(crate) fn inherit_nix_hashes(&mut self, old: &Image) {
        for (system, platform) in &mut self.arch {
            if let Some(old) = old.arch.get(system).filter(|o| o.digest == platform.digest) {
                platform.nix_hash = old.nix_hash.clone();
            }
        }
    }

    pub(crate) fn digest(&self, system: &str) -> Option<&str> {
        self.digests()
            .find_map(|(s, digest)| (s == system).then_some(digest))
    }

    /// `(system, digest)` of every recorded platform, in system order.
    pub(crate) fn digests(&self) -> impl Iterator<Item = (&str, &str)> {
        self.arch
            .iter()
            .map(|(system, platform)| (system.as_str(), platform.digest.as_str()))
    }
}

#[cfg(test)]
impl Image {
    pub(crate) fn systems(&self) -> Vec<&str> {
        self.arch.keys().map(String::as_str).collect()
    }

    /// `(digest, os, architecture)` of a system.
    pub(crate) fn platform(&self, system: &str) -> Option<(&str, &str, &str)> {
        self.arch
            .get(system)
            .map(|p| (p.digest.as_str(), p.os.as_str(), p.architecture.as_str()))
    }
}

/// One platform of an image, as `dockerTools.pullImage` fetches it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct PullSpec {
    /// `<registry>/<repository>`
    pub(crate) name: String,
    pub(crate) tag: String,
    pub(crate) digest: String,
    pub(crate) os: String,
    pub(crate) architecture: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Platform {
    digest: String,
    os: String,
    architecture: String,
    /// SRI hash of the `dockerTools.pullImage` archive; only for images pulled into the store.
    #[serde(rename = "nixHash", default, skip_serializing_if = "Option::is_none")]
    nix_hash: Option<String>,
}

impl Platform {
    pub(crate) fn new(digest: String, os: String, architecture: String) -> Self {
        Self {
            digest,
            os,
            architecture,
            nix_hash: None,
        }
    }

    pub(crate) fn nix_hash(&self) -> Option<&str> {
        self.nix_hash.as_deref()
    }
}

#[derive(Debug)]
pub(crate) enum LockfileError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse(serde_json::Error),
    UnsupportedVersion(u32),
    InvalidDigest {
        image: String,
        system: String,
        digest: String,
    },
    InvalidNixHash {
        image: String,
        system: String,
        hash: String,
    },
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot access {}: {source}", path.display()),
            Self::Parse(e) => write!(f, "invalid lockfile: {e}"),
            Self::UnsupportedVersion(v) => {
                write!(
                    f,
                    "unsupported lockfile version {v} (expected {SUPPORTED_VERSION})"
                )
            }
            Self::InvalidDigest {
                image,
                system,
                digest,
            } => {
                write!(f, "invalid digest '{digest}' for {image} ({system})")
            }
            Self::InvalidNixHash {
                image,
                system,
                hash,
            } => {
                write!(f, "invalid nixHash '{hash}' for {image} ({system})")
            }
        }
    }
}

impl std::error::Error for LockfileError {}

impl Lockfile {
    fn empty() -> Self {
        Self {
            version: SUPPORTED_VERSION,
            images: BTreeMap::new(),
        }
    }

    pub(crate) fn parse(json: &str) -> Result<Self, LockfileError> {
        let lock: Self = serde_json::from_str(json).map_err(LockfileError::Parse)?;
        if lock.version != SUPPORTED_VERSION {
            return Err(LockfileError::UnsupportedVersion(lock.version));
        }
        for (image, entry) in &lock.images {
            for (system, platform) in &entry.arch {
                if !is_valid_digest(&platform.digest) {
                    return Err(LockfileError::InvalidDigest {
                        image: image.clone(),
                        system: system.clone(),
                        digest: platform.digest.clone(),
                    });
                }
                if let Some(hash) = platform.nix_hash.as_deref().filter(|h| !is_valid_sri(h)) {
                    return Err(LockfileError::InvalidNixHash {
                        image: image.clone(),
                        system: system.clone(),
                        hash: hash.to_owned(),
                    });
                }
            }
        }
        Ok(lock)
    }

    /// A missing file is an empty lockfile: every declared image is then reported missing.
    pub(crate) fn read(path: &Path) -> Result<Self, LockfileError> {
        match std::fs::read_to_string(path) {
            Ok(json) => Self::parse(&json),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(source) => Err(LockfileError::Io {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    /// Keys that are absent or have no entry for `system`. Entries not in `keys` are ignored.
    pub(crate) fn missing<'a>(
        &self,
        keys: impl IntoIterator<Item = &'a str>,
        system: &str,
    ) -> Vec<String> {
        keys.into_iter()
            .filter(|key| {
                self.images
                    .get(*key)
                    .is_none_or(|image| !image.arch.contains_key(system))
            })
            .map(str::to_owned)
            .collect()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &str> {
        self.images.keys().map(String::as_str)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Image> {
        self.images.get(key)
    }

    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut Image> {
        self.images.get_mut(key)
    }

    pub(crate) fn insert(&mut self, key: String, image: Image) {
        self.images.insert(key, image);
    }

    /// Drops entries for which `keep` is false; returns the dropped keys.
    pub(crate) fn prune(&mut self, keep: impl Fn(&str) -> bool) -> Vec<String> {
        let dropped: Vec<String> = self
            .images
            .keys()
            .filter(|key| !keep(key))
            .cloned()
            .collect();
        for key in &dropped {
            self.images.remove(key);
        }
        dropped
    }

    /// Deterministic output: sorted keys, fixed field order, 2-space indent, final newline.
    pub(crate) fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("lockfile is serializable");
        json.push('\n');
        json
    }

    /// Atomic write (temporary file + rename), skipped when the content is unchanged.
    /// Returns whether the file was written.
    pub(crate) fn write(&self, path: &Path) -> Result<bool, LockfileError> {
        let json = self.to_json();
        if std::fs::read_to_string(path).is_ok_and(|current| current == json) {
            return Ok(false);
        }
        let io_err = |source| LockfileError::Io {
            path: path.to_path_buf(),
            source,
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let tmp = path.with_file_name(format!(".{name}.tmp"));
        std::fs::write(&tmp, &json).map_err(io_err)?;
        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(io_err(e));
        }
        Ok(true)
    }
}

/// Nix system for an OCI platform (table of BIBLE §2). Linux only; unlisted platforms map to `None`.
pub(crate) fn nix_system(
    os: &str,
    architecture: &str,
    variant: Option<&str>,
) -> Option<&'static str> {
    if os != "linux" {
        return None;
    }
    match (architecture, variant) {
        ("amd64", _) => Some("x86_64-linux"),
        ("arm64", None | Some("v8")) => Some("aarch64-linux"),
        ("arm", Some("v7")) => Some("armv7l-linux"),
        ("riscv64", _) => Some("riscv64-linux"),
        _ => None,
    }
}

pub(crate) fn is_valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `sha256-<base64 of 32 bytes>`, the form Nix takes as `sha256` (44 base64 characters, one `=`).
pub(crate) fn is_valid_sri(hash: &str) -> bool {
    hash.strip_prefix("sha256-").is_some_and(|b64| {
        b64.len() == 44
            && b64.ends_with('=')
            && b64[..43]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
            // 32 bytes leave 4 unused bits in the last character.
            && matches!(b64.as_bytes()[42], b'A' | b'E' | b'I' | b'M' | b'Q' | b'U' | b'Y' | b'c' | b'g' | b'k' | b'o' | b's' | b'w' | b'0' | b'4' | b'8')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    const HEX: &str = "d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499abcd";

    fn sample(system: &str) -> String {
        format!(
            r#"{{
  "version": 1,
  "images": {{
    "docker.io/library/caddy:2-alpine": {{
      "registry": "docker.io",
      "repository": "library/caddy",
      "tag": "2-alpine",
      "arch": {{
        "{system}": {{ "digest": "sha256:{HEX}", "os": "linux", "architecture": "amd64" }}
      }}
    }}
  }}
}}"#
        )
    }

    #[test]
    fn parses_valid_lockfile_and_roundtrips() {
        let lock = Lockfile::parse(&sample("x86_64-linux")).unwrap();
        let again = Lockfile::parse(&serde_json::to_string(&lock).unwrap()).unwrap();
        assert_eq!(lock, again);
    }

    #[test]
    fn rejects_unsupported_version() {
        let err = Lockfile::parse(r#"{"version": 2, "images": {}}"#).unwrap_err();
        assert!(matches!(err, LockfileError::UnsupportedVersion(2)));
    }

    #[test]
    fn rejects_malformed_digests() {
        for bad in [
            "d8a26e8499".to_string(),
            "sha256:abc".to_string(),
            format!("sha256:{}", HEX.to_uppercase()),
            format!("sha512:{HEX}"),
            format!("sha256:{HEX}0"),
        ] {
            let json = sample("x86_64-linux").replace(&format!("sha256:{HEX}"), &bad);
            let err = Lockfile::parse(&json).unwrap_err();
            assert!(matches!(err, LockfileError::InvalidDigest { .. }), "{bad}");
        }
    }

    #[test]
    fn rejects_unknown_fields_and_malformed_json() {
        assert!(matches!(
            Lockfile::parse(r#"{"version": 1, "images": {}, "extra": 1}"#),
            Err(LockfileError::Parse(_))
        ));
        assert!(matches!(Lockfile::parse("{"), Err(LockfileError::Parse(_))));
    }

    #[test]
    fn read_missing_file_is_empty() {
        let dir = temp_dir("lockfile-read-missing");
        let lock = Lockfile::read(&dir.join("pin-oci.lock")).unwrap();
        assert_eq!(lock, Lockfile::empty());
    }

    #[test]
    fn read_reports_io_errors_other_than_not_found() {
        let dir = temp_dir("lockfile-read-dir");
        assert!(matches!(
            Lockfile::read(&dir),
            Err(LockfileError::Io { .. })
        ));
    }

    #[test]
    fn read_parses_file_content() {
        let dir = temp_dir("lockfile-read-ok");
        let path = dir.join("pin-oci.lock");
        std::fs::write(&path, sample("x86_64-linux")).unwrap();
        assert_eq!(
            Lockfile::read(&path).unwrap(),
            Lockfile::parse(&sample("x86_64-linux")).unwrap()
        );
    }

    #[test]
    fn missing_reports_absent_key_and_absent_architecture() {
        let lock = Lockfile::parse(&sample("x86_64-linux")).unwrap();
        let key = "docker.io/library/caddy:2-alpine";
        let other = "docker.io/library/redis:7";

        assert!(lock.missing([key], "x86_64-linux").is_empty());
        assert_eq!(lock.missing([key, other], "x86_64-linux"), vec![other]);
        assert_eq!(lock.missing([key], "aarch64-linux"), vec![key]);
    }

    #[test]
    fn missing_ignores_extra_lock_entries_and_handles_no_targets() {
        let lock = Lockfile::parse(&sample("x86_64-linux")).unwrap();
        assert!(lock.missing([], "x86_64-linux").is_empty());
        assert_eq!(
            Lockfile::empty().missing(["a:1"], "x86_64-linux"),
            vec!["a:1"]
        );
    }

    fn image(tag: &str, systems: &[&str]) -> Image {
        let arch = systems
            .iter()
            .map(|s| {
                (
                    s.to_string(),
                    Platform::new(format!("sha256:{HEX}"), "linux".into(), "amd64".into()),
                )
            })
            .collect();
        Image::new("docker.io".into(), "library/x".into(), tag.into(), arch)
    }

    #[test]
    fn nix_system_follows_the_platform_table() {
        for (os, arch, variant, expected) in [
            ("linux", "amd64", None, Some("x86_64-linux")),
            ("linux", "arm64", None, Some("aarch64-linux")),
            ("linux", "arm64", Some("v8"), Some("aarch64-linux")),
            ("linux", "arm", Some("v7"), Some("armv7l-linux")),
            ("linux", "riscv64", None, Some("riscv64-linux")),
            ("linux", "arm", Some("v6"), None),
            ("linux", "arm", Some("v5"), None),
            ("linux", "arm", None, None),
            ("linux", "arm64", Some("v9"), None),
            ("linux", "ppc64le", None, None),
            ("linux", "s390x", None, None),
            ("windows", "amd64", None, None),
            ("unknown", "unknown", None, None),
        ] {
            assert_eq!(
                nix_system(os, arch, variant),
                expected,
                "{os}/{arch}/{variant:?}"
            );
        }
    }

    #[test]
    fn serialization_is_deterministic_and_independent_of_insertion_order() {
        let build = |order: &[&str]| {
            let mut lock = Lockfile::empty();
            for key in order {
                lock.insert(
                    (*key).to_owned(),
                    image(key, &["x86_64-linux", "aarch64-linux"]),
                );
            }
            lock.to_json()
        };
        let a = build(&["b:1", "a:1", "c:1"]);
        assert_eq!(a, build(&["c:1", "b:1", "a:1"]));
        assert_eq!(a, build(&["b:1", "a:1", "c:1"]));
        assert!(a.ends_with("}\n"));
        let (pa, pb) = (a.find("\"a:1\"").unwrap(), a.find("\"b:1\"").unwrap());
        assert!(pa < pb);
        assert!(a.find("aarch64-linux").unwrap() < a.find("x86_64-linux").unwrap());
    }

    #[test]
    fn serialization_matches_the_documented_field_order_and_reparses() {
        let mut lock = Lockfile::empty();
        lock.insert("k:1".into(), image("1", &["x86_64-linux"]));
        let json = lock.to_json();
        let positions: Vec<usize> = [
            "version",
            "images",
            "registry",
            "repository",
            "tag",
            "arch",
            "digest",
            "os",
            "architecture",
        ]
        .iter()
        .map(|f| json.find(&format!("\"{f}\"")).unwrap())
        .collect();
        assert!(positions.windows(2).all(|w| w[0] < w[1]), "{json}");
        assert_eq!(Lockfile::parse(&json).unwrap(), lock);
    }

    #[test]
    fn prune_drops_only_entries_rejected_by_the_predicate() {
        let mut lock = Lockfile::empty();
        lock.insert("keep:1".into(), image("1", &["x86_64-linux"]));
        lock.insert("stale:1".into(), image("1", &["x86_64-linux"]));
        assert_eq!(lock.prune(|k| k == "keep:1"), vec!["stale:1"]);
        assert!(lock.missing(["keep:1"], "x86_64-linux").is_empty());
        assert_eq!(lock.missing(["stale:1"], "x86_64-linux"), vec!["stale:1"]);
    }

    #[test]
    fn write_is_atomic_idempotent_and_leaves_no_temporary_file() {
        let dir = temp_dir("lockfile-write");
        let path = dir.join("pin-oci.lock");
        let mut lock = Lockfile::empty();
        lock.insert("k:1".into(), image("1", &["x86_64-linux"]));

        assert!(lock.write(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), lock.to_json());
        assert!(
            !lock.write(&path).unwrap(),
            "identical content is not rewritten"
        );
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("pin-oci.lock")]);
    }

    #[test]
    fn write_failure_leaves_the_existing_file_untouched() {
        let dir = temp_dir("lockfile-write-fail");
        let path = dir.join("pin-oci.lock");
        std::fs::write(&path, "previous").unwrap();
        // The temporary file name is taken by a directory: the write must fail.
        std::fs::create_dir(dir.join(".pin-oci.lock.tmp")).unwrap();

        let mut lock = Lockfile::empty();
        lock.insert("k:1".into(), image("1", &["x86_64-linux"]));
        assert!(matches!(lock.write(&path), Err(LockfileError::Io { .. })));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "previous");
    }

    const SRI: &str = "sha256-4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnHk=";

    #[test]
    fn nix_hash_is_optional_roundtrips_and_is_omitted_when_absent() {
        let without = Lockfile::parse(&sample("x86_64-linux")).unwrap();
        assert!(!without.to_json().contains("nixHash"));

        let json = sample("x86_64-linux").replace(
            r#""architecture": "amd64" }"#,
            &format!(r#""architecture": "amd64", "nixHash": "{SRI}" }}"#),
        );
        let with = Lockfile::parse(&json).unwrap();
        let key = "docker.io/library/caddy:2-alpine";
        assert_eq!(with.get(key).unwrap().nix_hash("x86_64-linux"), Some(SRI));
        assert_eq!(Lockfile::parse(&with.to_json()).unwrap(), with);
        let out = with.to_json();
        assert!(out.find("\"architecture\"").unwrap() < out.find("\"nixHash\"").unwrap());
    }

    #[test]
    fn rejects_malformed_nix_hashes() {
        for bad in [
            "",
            "sha256-",
            "sha256-abc=",
            "sha512-4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnHk=",
            "4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnHk=",
            "sha256-4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnHk",
            "sha256-4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnH!=",
            "sha256-4Y35qO8RkPzUQ4kJmDIdq0EY3WR5G8wXfA9b2QpCnHl=",
            HEX,
        ] {
            let json = sample("x86_64-linux").replace(
                r#""architecture": "amd64" }"#,
                &format!(r#""architecture": "amd64", "nixHash": "{bad}" }}"#),
            );
            assert!(
                matches!(
                    Lockfile::parse(&json),
                    Err(LockfileError::InvalidNixHash { .. })
                ),
                "{bad}"
            );
        }
    }
}
