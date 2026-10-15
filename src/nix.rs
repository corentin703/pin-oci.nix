use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

/// Evaluated on `nixosConfigurations.<host>.config`. Never depends on the lockfile.
const CHECK_APPLY: &str = "c: { system = c.nixpkgs.hostPlatform.system; \
                           targets = builtins.attrNames c.virtualisation.pin-oci.lockTargets; }";

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Declared {
    pub(crate) system: String,
    pub(crate) targets: Vec<String>,
}

#[derive(Debug)]
pub(crate) enum NixError {
    MissingHost(String),
    Spawn(io::Error),
    Failed { code: Option<i32>, stderr: String },
    Parse(serde_json::Error),
}

impl fmt::Display for NixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingHost(flake) => {
                write!(
                    f,
                    "no host in flake reference '{flake}' (expected <flake>#<host>)"
                )
            }
            Self::Spawn(e) => write!(f, "cannot run nix: {e}"),
            Self::Failed { code, stderr } => match code {
                Some(c) => write!(f, "nix eval failed (exit {c}): {}", stderr.trim_end()),
                None => write!(f, "nix eval killed by a signal: {}", stderr.trim_end()),
            },
            Self::Parse(e) => write!(f, "unexpected nix eval output: {e}"),
        }
    }
}

impl std::error::Error for NixError {}

/// Boundary around the `nix` binary, stubbed in tests.
pub(crate) trait Evaluator {
    fn eval_json(&self, installable: &str, apply: &str) -> Result<String, NixError>;
}

pub(crate) struct NixCli;

impl Evaluator for NixCli {
    fn eval_json(&self, installable: &str, apply: &str) -> Result<String, NixError> {
        let output = Command::new("nix")
            .args(["eval", "--json", installable, "--apply", apply])
            .output()
            .map_err(NixError::Spawn)?;
        if !output.status.success() {
            return Err(NixError::Failed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// `<flake>#<host>` -> `<flake>#nixosConfigurations."<host>".config`
fn installable(flake: &str) -> Result<String, NixError> {
    match flake.split_once('#') {
        Some((base, host)) if !host.is_empty() => {
            Ok(format!("{base}#nixosConfigurations.\"{host}\".config"))
        }
        _ => Err(NixError::MissingHost(flake.to_owned())),
    }
}

/// Local directory of a flake reference, if it has one.
pub(crate) fn flake_root(flake: &str) -> Option<PathBuf> {
    let base = flake.split_once('#').map_or(flake, |(base, _)| base);
    let path = base.strip_prefix("path:").unwrap_or(base);
    (!path.is_empty() && !path.contains(':')).then(|| PathBuf::from(path))
}

pub(crate) fn declared(evaluator: &dyn Evaluator, flake: &str) -> Result<Declared, NixError> {
    let json = evaluator.eval_json(&installable(flake)?, CHECK_APPLY)?;
    serde_json::from_str(&json).map_err(NixError::Parse)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::StubEvaluator;

    #[test]
    fn installable_targets_the_host_configuration() {
        assert_eq!(
            installable("/etc/nixos#box").unwrap(),
            "/etc/nixos#nixosConfigurations.\"box\".config"
        );
        assert_eq!(
            installable(".#a.b").unwrap(),
            ".#nixosConfigurations.\"a.b\".config"
        );
    }

    #[test]
    fn installable_requires_a_host() {
        assert!(matches!(installable("."), Err(NixError::MissingHost(_))));
        assert!(matches!(installable(".#"), Err(NixError::MissingHost(_))));
    }

    #[test]
    fn flake_root_of_local_and_remote_references() {
        assert_eq!(flake_root("."), Some(PathBuf::from(".")));
        assert_eq!(
            flake_root("/etc/nixos#box"),
            Some(PathBuf::from("/etc/nixos"))
        );
        assert_eq!(
            flake_root("path:/etc/nixos#box"),
            Some(PathBuf::from("/etc/nixos"))
        );
        assert_eq!(flake_root("github:owner/repo#box"), None);
        assert_eq!(flake_root("git+https://example.org/r#box"), None);
        assert_eq!(flake_root("#box"), None);
    }

    #[test]
    fn declared_parses_evaluation_output_and_queries_lock_targets() {
        let stub = StubEvaluator::ok(r#"{"system":"x86_64-linux","targets":["a:1","b:2"]}"#);
        let got = declared(&stub, ".#box").unwrap();
        assert_eq!(
            got,
            Declared {
                system: "x86_64-linux".into(),
                targets: vec!["a:1".into(), "b:2".into()]
            }
        );
        let calls = stub.calls.borrow();
        assert_eq!(calls[0].0, ".#nixosConfigurations.\"box\".config");
        assert!(calls[0].1.contains("lockTargets") && calls[0].1.contains("hostPlatform.system"));
    }

    #[test]
    fn declared_rejects_malformed_output() {
        for bad in [
            "not json",
            r#"{"system":"x"}"#,
            r#"{"system":1,"targets":[]}"#,
        ] {
            let stub = StubEvaluator::ok(bad);
            assert!(
                matches!(declared(&stub, ".#box"), Err(NixError::Parse(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn declared_propagates_evaluator_failure_without_calling_nix_when_host_missing() {
        let stub = StubEvaluator::failing("boom");
        assert!(matches!(
            declared(&stub, ".#box"),
            Err(NixError::Failed { .. })
        ));

        let stub = StubEvaluator::ok("{}");
        assert!(matches!(
            declared(&stub, "."),
            Err(NixError::MissingHost(_))
        ));
        assert!(stub.calls.borrow().is_empty());
    }
}
