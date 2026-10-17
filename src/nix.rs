use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

use crate::lockfile::Lockfile;

/// Never depends on the lockfile; also exercised by the Nix module tests.
const APPLY: &str = include_str!("apply.nix");
/// Same contract as `APPLY`, but null for a host that does not use pin-oci.
const APPLY_OPTIONAL: &str = include_str!("apply-optional.nix");
const APPLY_HOST_NAMES: &str = "builtins.attrNames";

/// What one host declares, as printed by `APPLY`.
#[derive(Debug, PartialEq, Eq, Deserialize)]
struct HostDeclared {
    system: String,
    /// Canonical key -> target.
    targets: BTreeMap<String, Target>,
}

/// The union of what the evaluated hosts declare. The lockfile knows platforms, not hosts.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Declared {
    /// Canonical key -> target.
    pub(crate) targets: BTreeMap<String, Target>,
    /// Canonical key -> platforms of the hosts declaring it.
    pub(crate) systems: BTreeMap<String, BTreeSet<String>>,
    /// Canonical key -> hosts declaring it.
    pub(crate) hosts: BTreeMap<String, BTreeSet<String>>,
    /// Platforms of every evaluated host.
    pub(crate) all_systems: BTreeSet<String>,
    /// Hosts found without pin-oci enabled; they contribute nothing.
    pub(crate) skipped: Vec<String>,
}

impl Declared {
    /// `None` is a host that does not use pin-oci.
    fn merge(hosts: Vec<(String, Option<HostDeclared>)>) -> Self {
        let mut out = Self {
            targets: BTreeMap::new(),
            systems: BTreeMap::new(),
            hosts: BTreeMap::new(),
            all_systems: BTreeSet::new(),
            skipped: Vec::new(),
        };
        for (host, declared) in hosts {
            let Some(declared) = declared else {
                out.skipped.push(host);
                continue;
            };
            out.all_systems.insert(declared.system.clone());
            for (key, target) in declared.targets {
                out.systems
                    .entry(key.clone())
                    .or_default()
                    .insert(declared.system.clone());
                out.hosts
                    .entry(key.clone())
                    .or_default()
                    .insert(host.clone());
                out.targets.insert(key, target);
            }
        }
        out
    }

    /// Declared keys with no lockfile entry for the platform of at least one declaring host.
    pub(crate) fn unlocked(&self, lock: &Lockfile) -> Vec<String> {
        self.systems
            .iter()
            .filter(|(key, systems)| {
                systems
                    .iter()
                    .any(|system| !lock.missing([key.as_str()], system).is_empty())
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Evaluated platforms, for messages.
    pub(crate) fn systems_label(&self) -> String {
        self.all_systems
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// One line naming the hosts that were left out, empty when there are none.
    pub(crate) fn skipped_note(&self) -> String {
        if self.skipped.is_empty() {
            return String::new();
        }
        format!(
            "
pin-oci: ignored, pin-oci not enabled: {}",
            self.skipped.join(", ")
        )
    }
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
pub(crate) struct Target {
    pub(crate) registry: String,
    pub(crate) repository: String,
    pub(crate) tag: String,
}

#[derive(Debug)]
pub(crate) enum NixError {
    /// No `nixosConfigurations` host, or none of them uses pin-oci.
    NoHost(String),
    /// Every host whose evaluation failed, with the evaluator's message. Nothing was merged.
    Hosts(Vec<(String, String)>),
    Spawn(io::Error),
    Failed {
        code: Option<i32>,
        stderr: String,
    },
    Parse(serde_json::Error),
}

impl fmt::Display for NixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHost(flake) => write!(
                f,
                "no nixosConfigurations host of '{flake}' uses pin-oci (or pass --flake <flake>#<host>)"
            ),
            Self::Hosts(errors) => {
                write!(f, "cannot evaluate {} host(s):", errors.len())?;
                for (host, e) in errors {
                    write!(f, "\n  - {host}: {e}")?;
                }
                Ok(())
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

/// `<flake>`, `<host>` -> `<flake>#nixosConfigurations."<host>".config`
fn installable(base: &str, host: &str) -> String {
    format!("{base}#nixosConfigurations.\"{host}\".config")
}

fn parse<T: for<'de> Deserialize<'de>>(json: &str) -> Result<T, NixError> {
    serde_json::from_str(json).map_err(NixError::Parse)
}

/// Local directory of a flake reference, if it has one.
pub(crate) fn flake_root(flake: &str) -> Option<PathBuf> {
    let base = flake.split_once('#').map_or(flake, |(base, _)| base);
    let path = base.strip_prefix("path:").unwrap_or(base);
    (!path.is_empty() && !path.contains(':')).then(|| PathBuf::from(path))
}

/// `<flake>#<host>` declares that host. A reference without a host (the default `.`) declares
/// the union of every `nixosConfigurations` host that has pin-oci enabled. All or nothing:
/// any host that fails to evaluate fails the whole call.
pub(crate) fn declared(evaluator: &dyn Evaluator, flake: &str) -> Result<Declared, NixError> {
    let (base, host) = flake.split_once('#').unwrap_or((flake, ""));
    if !host.is_empty() {
        let one: HostDeclared = parse(&evaluator.eval_json(&installable(base, host), APPLY)?)?;
        return Ok(Declared::merge(vec![(host.to_owned(), Some(one))]));
    }

    let names: Vec<String> =
        parse(&evaluator.eval_json(&format!("{base}#nixosConfigurations"), APPLY_HOST_NAMES)?)?;
    let mut hosts = Vec::new();
    let mut errors = Vec::new();
    for name in names {
        match evaluator
            .eval_json(&installable(base, &name), APPLY_OPTIONAL)
            .and_then(|json| parse::<Option<HostDeclared>>(&json))
        {
            Ok(declared) => hosts.push((name, declared)),
            Err(e) => errors.push((name, e.to_string())),
        }
    }
    if !errors.is_empty() {
        return Err(NixError::Hosts(errors));
    }
    let merged = Declared::merge(hosts);
    if merged.all_systems.is_empty() {
        return Err(NixError::NoHost(base.to_owned()));
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::StubEvaluator;

    #[test]
    fn installable_targets_the_host_configuration() {
        assert_eq!(
            installable("/etc/nixos", "box"),
            "/etc/nixos#nixosConfigurations.\"box\".config"
        );
        assert_eq!(
            installable(".", "a.b"),
            ".#nixosConfigurations.\"a.b\".config"
        );
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
        let stub = StubEvaluator::ok(
            r#"{"system":"x86_64-linux","targets":{"docker.io/library/a:1":
                {"registry":"docker.io","repository":"library/a","tag":"1"}}}"#,
        );
        let got = declared(&stub, ".#box").unwrap();
        assert_eq!(got.all_systems, set(&["x86_64-linux"]));
        assert_eq!(got.hosts["docker.io/library/a:1"], set(&["box"]));
        assert_eq!(
            got.targets["docker.io/library/a:1"],
            Target {
                registry: "docker.io".into(),
                repository: "library/a".into(),
                tag: "1".into()
            }
        );
        let calls = stub.calls.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, ".#nixosConfigurations.\"box\".config");
        assert!(calls[0].1.contains("lockTargets") && calls[0].1.contains("pin-oci.system"));
    }

    #[test]
    fn declared_rejects_malformed_output() {
        for bad in [
            "not json",
            r#"{"system":"x"}"#,
            r#"{"system":1,"targets":{}}"#,
            r#"{"system":"x","targets":["a:1"]}"#,
            r#"{"system":"x","targets":{"a:1":{"registry":"r"}}}"#,
        ] {
            let stub = StubEvaluator::ok(bad);
            assert!(
                matches!(declared(&stub, ".#box"), Err(NixError::Parse(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn declared_propagates_evaluator_failure() {
        let stub = StubEvaluator::failing("boom");
        assert!(matches!(
            declared(&stub, ".#box"),
            Err(NixError::Failed { .. })
        ));
        assert!(matches!(declared(&stub, "."), Err(NixError::Failed { .. })));
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    fn host_json(system: &str, keys: &[&str]) -> String {
        let targets: Vec<String> = keys
            .iter()
            .map(|k| format!(r#""{k}":{{"registry":"r","repository":"p","tag":"t"}}"#))
            .collect();
        format!(
            r#"{{"system":"{system}","targets":{{{}}}}}"#,
            targets.join(",")
        )
    }

    const NAMES: &str = ".#nixosConfigurations";
    const A: &str = ".#nixosConfigurations.\"a\".config";
    const B: &str = ".#nixosConfigurations.\"b\".config";
    const C: &str = ".#nixosConfigurations.\"c\".config";

    #[test]
    fn without_a_host_every_host_is_evaluated_and_merged_into_a_union() {
        let stub = StubEvaluator::routed()
            .route(NAMES, r#"["a","b"]"#)
            .route(A, &host_json("x86_64-linux", &["k1:1", "shared:1"]))
            .route(B, &host_json("aarch64-linux", &["shared:1", "k2:1"]));
        let got = declared(&stub, ".").unwrap();

        assert_eq!(
            got.targets.keys().map(String::as_str).collect::<Vec<_>>(),
            ["k1:1", "k2:1", "shared:1"]
        );
        assert_eq!(got.hosts["shared:1"], set(&["a", "b"]));
        assert_eq!(got.hosts["k1:1"], set(&["a"]));
        assert_eq!(
            got.systems["shared:1"],
            set(&["x86_64-linux", "aarch64-linux"])
        );
        assert_eq!(got.systems["k2:1"], set(&["aarch64-linux"]));
        assert_eq!(got.systems_label(), "aarch64-linux, x86_64-linux");
        assert!(got.skipped.is_empty() && got.skipped_note().is_empty());
        let calls = stub.calls.borrow();
        assert_eq!(calls[0].1, "builtins.attrNames");
        assert!(calls[1..]
            .iter()
            .all(|(_, apply)| apply.contains("p != null")));
    }

    #[test]
    fn a_trailing_hash_means_every_host_too() {
        let stub = StubEvaluator::routed()
            .route(NAMES, r#"["a"]"#)
            .route(A, &host_json("x86_64-linux", &["k1:1"]));
        assert!(declared(&stub, ".#").is_ok());
    }

    #[test]
    fn hosts_without_pin_oci_are_skipped_and_named() {
        let stub = StubEvaluator::routed()
            .route(NAMES, r#"["a","b","c"]"#)
            .route(A, &host_json("x86_64-linux", &["k1:1"]))
            .route(B, "null")
            .route(C, "null");
        let got = declared(&stub, ".").unwrap();
        assert_eq!(got.skipped, ["b", "c"]);
        assert_eq!(
            got.skipped_note(),
            "\npin-oci: ignored, pin-oci not enabled: b, c"
        );
        assert_eq!(got.hosts["k1:1"], set(&["a"]));
    }

    #[test]
    fn no_host_or_no_host_using_pin_oci_is_an_error() {
        let none = StubEvaluator::routed().route(NAMES, "[]");
        assert!(matches!(declared(&none, "."), Err(NixError::NoHost(_))));

        let unused = StubEvaluator::routed()
            .route(NAMES, r#"["a","b"]"#)
            .route(A, "null")
            .route(B, "null");
        assert!(matches!(declared(&unused, "."), Err(NixError::NoHost(_))));

        let absent = StubEvaluator::routed().route_failing(NAMES, "does not provide attribute");
        assert!(matches!(
            declared(&absent, "."),
            Err(NixError::Failed { .. })
        ));
    }

    #[test]
    fn one_failing_host_fails_the_whole_union_and_all_failures_are_reported() {
        let stub = StubEvaluator::routed()
            .route(NAMES, r#"["a","b","c"]"#)
            .route(A, &host_json("x86_64-linux", &["k1:1"]))
            .route_failing(B, "boom b")
            .route_failing(C, "boom c");
        let err = declared(&stub, ".").unwrap_err();
        let NixError::Hosts(errors) = &err else {
            panic!("{err}")
        };
        assert_eq!(errors.len(), 2);
        let message = err.to_string();
        assert!(message.contains("b: ") && message.contains("boom b"));
        assert!(message.contains("c: ") && message.contains("boom c"));
    }

    #[test]
    fn a_host_given_explicitly_does_not_list_the_others() {
        let stub = StubEvaluator::routed().route(B, &host_json("x86_64-linux", &["k:1"]));
        declared(&stub, ".#b").unwrap();
        assert_eq!(stub.calls.borrow().len(), 1);
    }

    #[test]
    fn unlocked_needs_an_entry_for_every_platform_of_every_declaring_host() {
        let lock = Lockfile::parse(
            r#"{"version":1,"images":{"shared:1":{"registry":"r","repository":"p","tag":"t",
            "arch":{"x86_64-linux":{"digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","os":"linux","architecture":"amd64"}}}}}"#,
        )
        .unwrap();
        let both = Declared::merge(vec![
            (
                "a".into(),
                Some(parsed(&host_json("x86_64-linux", &["shared:1"]))),
            ),
            (
                "b".into(),
                Some(parsed(&host_json("aarch64-linux", &["shared:1"]))),
            ),
        ]);
        assert_eq!(both.unlocked(&lock), ["shared:1"], "aarch64 is not locked");

        let only_a = Declared::merge(vec![(
            "a".into(),
            Some(parsed(&host_json("x86_64-linux", &["shared:1"]))),
        )]);
        assert!(only_a.unlocked(&lock).is_empty());
    }

    fn parsed(json: &str) -> HostDeclared {
        serde_json::from_str(json).unwrap()
    }
}
