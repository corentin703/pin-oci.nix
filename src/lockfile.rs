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
struct Image {
    registry: String,
    repository: String,
    tag: String,
    arch: BTreeMap<String, Platform>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Platform {
    digest: String,
    os: String,
    architecture: String,
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
}

impl fmt::Display for LockfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "cannot read {}: {source}", path.display()),
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
}

fn is_valid_digest(digest: &str) -> bool {
    digest.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
}
