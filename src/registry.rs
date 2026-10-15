use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::process::Command;

use serde::Deserialize;

use crate::lockfile::{is_valid_digest, nix_system, Image, Platform};
use crate::nix::Target;

#[derive(Debug)]
pub(crate) enum RegistryError {
    Spawn(io::Error),
    Failed { code: Option<i32>, stderr: String },
    Parse(serde_json::Error),
    InvalidDigest(String),
    UnsupportedPlatform(String),
    AmbiguousPlatform(String),
    NoSupportedPlatform,
    MissingHostPlatform(String),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run skopeo: {e}"),
            Self::Failed { code, stderr } => match code {
                Some(c) => write!(f, "skopeo failed (exit {c}): {}", stderr.trim_end()),
                None => write!(f, "skopeo killed by a signal: {}", stderr.trim_end()),
            },
            Self::Parse(e) => write!(f, "unexpected skopeo output: {e}"),
            Self::InvalidDigest(d) => write!(f, "registry returned an invalid digest '{d}'"),
            Self::UnsupportedPlatform(p) => write!(f, "unsupported platform {p}"),
            Self::AmbiguousPlatform(s) => write!(f, "several manifests match {s}"),
            Self::NoSupportedPlatform => write!(f, "no supported linux platform in the image"),
            Self::MissingHostPlatform(s) => write!(f, "no manifest for the host platform {s}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Boundary around the `skopeo` binary, stubbed in tests.
pub(crate) trait Runtime {
    fn skopeo(&self, args: &[&str]) -> Result<String, RegistryError>;
}

pub(crate) struct SkopeoCli;

/// References are always fully qualified, so the host's registries.conf (search, mirrors) is
/// ignored: resolution must come from the canonical registry, and a v1 file is rejected by
/// recent skopeo versions. Credentials are not affected.
const REGISTRIES_CONF: &str = "/dev/null";

impl Runtime for SkopeoCli {
    fn skopeo(&self, args: &[&str]) -> Result<String, RegistryError> {
        let output = Command::new("skopeo")
            .args(["--registries-conf", REGISTRIES_CONF])
            .args(args)
            .output()
            .map_err(RegistryError::Spawn)?;
        if !output.status.success() {
            return Err(RegistryError::Failed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// `skopeo inspect --raw`: an index carries `manifests`, a single manifest does not.
#[derive(Deserialize)]
struct Raw {
    manifests: Option<Vec<IndexEntry>>,
}

#[derive(Deserialize)]
struct IndexEntry {
    digest: String,
    platform: Option<PlatformDesc>,
}

#[derive(Deserialize)]
struct PlatformDesc {
    os: String,
    architecture: String,
    variant: Option<String>,
}

/// `skopeo inspect --no-tags` on a single manifest.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Inspect {
    digest: String,
    architecture: String,
    os: String,
    variant: Option<String>,
}

struct Candidate {
    digest: String,
    os: String,
    architecture: String,
    variant: Option<String>,
}

/// Resolves a target to its per-platform digests.
///
/// In an index, entries that do not map to a Nix system (attestations, `s390x`, `arm` v6...) are
/// ignored. A single manifest has exactly one platform: if unsupported, resolution fails.
pub(crate) fn resolve(runtime: &dyn Runtime, target: &Target) -> Result<Image, RegistryError> {
    let reference = format!(
        "docker://{}/{}:{}",
        target.registry, target.repository, target.tag
    );
    let raw: Raw = parse(&runtime.skopeo(&["inspect", "--raw", &reference])?)?;

    let (candidates, strict) = match raw.manifests {
        Some(entries) => {
            let candidates = entries
                .into_iter()
                .filter_map(|e| {
                    e.platform.map(|p| Candidate {
                        digest: e.digest,
                        os: p.os,
                        architecture: p.architecture,
                        variant: p.variant,
                    })
                })
                .collect();
            (candidates, false)
        }
        None => {
            let single: Inspect = parse(&runtime.skopeo(&["inspect", "--no-tags", &reference])?)?;
            let candidate = Candidate {
                digest: single.digest,
                os: single.os,
                architecture: single.architecture,
                variant: single.variant,
            };
            (vec![candidate], true)
        }
    };

    let mut arch = BTreeMap::new();
    for c in candidates {
        let Some(system) = nix_system(&c.os, &c.architecture, c.variant.as_deref()) else {
            if strict {
                let variant = c.variant.map(|v| format!("/{v}")).unwrap_or_default();
                return Err(RegistryError::UnsupportedPlatform(format!(
                    "{}/{}{variant}",
                    c.os, c.architecture
                )));
            }
            continue;
        };
        if !is_valid_digest(&c.digest) {
            return Err(RegistryError::InvalidDigest(c.digest));
        }
        let platform = Platform::new(c.digest, c.os, c.architecture);
        if arch.insert(system.to_owned(), platform).is_some() {
            return Err(RegistryError::AmbiguousPlatform(system.to_owned()));
        }
    }
    if arch.is_empty() {
        return Err(RegistryError::NoSupportedPlatform);
    }
    Ok(Image::new(
        target.registry.clone(),
        target.repository.clone(),
        target.tag.clone(),
        arch,
    ))
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, RegistryError> {
    serde_json::from_str(json).map_err(RegistryError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::StubRuntime;

    const INDEX: &str = include_str!("../tests/fixtures/skopeo-index.json");
    const SINGLE_RAW: &str = include_str!("../tests/fixtures/skopeo-single-raw.json");
    const SINGLE_INSPECT: &str = include_str!("../tests/fixtures/skopeo-single-inspect.json");
    const HEX: &str = "d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499abcd";

    fn target() -> Target {
        Target {
            registry: "docker.io".into(),
            repository: "library/caddy".into(),
            tag: "2-alpine".into(),
        }
    }

    const REF: &str = "docker://docker.io/library/caddy:2-alpine";

    #[test]
    fn resolves_a_real_multi_platform_index_and_ignores_unmappable_entries() {
        let rt = StubRuntime::new().on(&["inspect", "--raw", REF], INDEX);
        let image = resolve(&rt, &target()).unwrap();

        assert_eq!(
            image.systems(),
            vec![
                "aarch64-linux",
                "armv7l-linux",
                "riscv64-linux",
                "x86_64-linux"
            ]
        );
        let (digest, os, architecture) = image.platform("x86_64-linux").unwrap();
        assert!(digest.starts_with("sha256:173b26306d71"));
        assert_eq!((os, architecture), ("linux", "amd64"));
        assert_eq!(rt.calls.borrow().len(), 1, "an index needs no second call");
    }

    #[test]
    fn resolves_a_single_manifest_through_inspect() {
        let rt = StubRuntime::new()
            .on(&["inspect", "--raw", REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", REF], SINGLE_INSPECT);
        let image = resolve(&rt, &target()).unwrap();

        assert_eq!(image.systems(), vec!["x86_64-linux"]);
        let (digest, _, _) = image.platform("x86_64-linux").unwrap();
        assert!(digest.starts_with("sha256:173b26306d71"));
    }

    #[test]
    fn single_manifest_with_unsupported_platform_is_an_error() {
        let inspect = format!(
            r#"{{"Digest":"sha256:{HEX}","Architecture":"arm","Os":"linux","Variant":"v6"}}"#
        );
        let rt = StubRuntime::new()
            .on(&["inspect", "--raw", REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", REF], &inspect);
        let err = resolve(&rt, &target()).unwrap_err();
        assert!(matches!(&err, RegistryError::UnsupportedPlatform(p) if p == "linux/arm/v6"));
    }

    #[test]
    fn index_without_any_supported_platform_is_an_error() {
        let index = format!(
            r#"{{"manifests":[
                {{"digest":"sha256:{HEX}","platform":{{"os":"unknown","architecture":"unknown"}}}},
                {{"digest":"sha256:{HEX}","platform":{{"os":"linux","architecture":"s390x"}}}},
                {{"digest":"sha256:{HEX}"}}]}}"#
        );
        let rt = StubRuntime::new().on(&["inspect", "--raw", REF], &index);
        assert!(matches!(
            resolve(&rt, &target()),
            Err(RegistryError::NoSupportedPlatform)
        ));
    }

    #[test]
    fn index_with_two_manifests_for_one_system_is_ambiguous() {
        let entry = format!(
            r#"{{"digest":"sha256:{HEX}","platform":{{"os":"linux","architecture":"amd64"}}}}"#
        );
        let index = format!(r#"{{"manifests":[{entry},{entry}]}}"#);
        let rt = StubRuntime::new().on(&["inspect", "--raw", REF], &index);
        assert!(matches!(
            resolve(&rt, &target()),
            Err(RegistryError::AmbiguousPlatform(_))
        ));
    }

    #[test]
    fn registry_digests_are_validated() {
        let index = r#"{"manifests":[{"digest":"sha256:short",
            "platform":{"os":"linux","architecture":"amd64"}}]}"#;
        let rt = StubRuntime::new().on(&["inspect", "--raw", REF], index);
        assert!(matches!(
            resolve(&rt, &target()),
            Err(RegistryError::InvalidDigest(_))
        ));
    }

    #[test]
    fn skopeo_failure_and_malformed_output_are_errors() {
        let rt = StubRuntime::new().failing(&["inspect", "--raw", REF], "manifest unknown");
        let err = resolve(&rt, &target()).unwrap_err();
        assert!(err.to_string().contains("manifest unknown"));

        let rt = StubRuntime::new().on(&["inspect", "--raw", REF], "not json");
        assert!(matches!(
            resolve(&rt, &target()),
            Err(RegistryError::Parse(_))
        ));

        let rt = StubRuntime::new()
            .on(&["inspect", "--raw", REF], SINGLE_RAW)
            .on(&["inspect", "--no-tags", REF], r#"{"Digest":"x"}"#);
        assert!(matches!(
            resolve(&rt, &target()),
            Err(RegistryError::Parse(_))
        ));
    }
}
