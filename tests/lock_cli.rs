//! End-to-end: `pin-oci lock` then `check`, against a fake `nix` and a fake `skopeo` on PATH.
//! The fake skopeo replays real captured outputs. Offline.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

const CADDY: &str = r#""docker.io/library/caddy:2-alpine":{"registry":"docker.io","repository":"library/caddy","tag":"2-alpine"}"#;
const SOLO: &str =
    r#""docker.io/acme/solo:1":{"registry":"docker.io","repository":"acme/solo","tag":"1"}"#;
const NOPE: &str =
    r#""docker.io/library/nope:1":{"registry":"docker.io","repository":"library/nope","tag":"1"}"#;

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pin-oci-lock-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("flake")).unwrap();
        Self { dir }
    }

    fn lockfile(&self) -> PathBuf {
        self.dir.join("flake/pin-oci.lock")
    }

    fn executable(&self, name: &str, body: &str) {
        let path = self.dir.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn nix(&self, system: &str, targets: &[&str]) {
        let json = format!(
            r#"{{"system":"{system}","targets":{{{}}}}}"#,
            targets.join(",")
        );
        self.executable("nix", &format!("printf '%s' '{json}'"));
    }

    fn skopeo(&self) {
        let log = self.dir.join("skopeo.log");
        self.executable(
            "skopeo",
            &format!(
                r#"echo "$*" >> '{log}'
case "$*" in
  "--registries-conf /dev/null "*) ;;
  *) echo "registries.conf must be in v2 format but is in v1" >&2; exit 1 ;;
esac
emit() {{ while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$1"; }}
case "$*" in
  *--raw*/library/caddy:*) emit '{f}/skopeo-index.json' ;;
  *--raw*/acme/solo:*) emit '{f}/skopeo-single-raw.json' ;;
  *--no-tags*/acme/solo:*) emit '{f}/skopeo-single-inspect.json' ;;
  *) echo "manifest unknown" >&2; exit 2 ;;
esac"#,
                log = log.display(),
                f = FIXTURES
            ),
        );
    }

    fn skopeo_calls(&self) -> usize {
        fs::read_to_string(self.dir.join("skopeo.log")).map_or(0, |log| log.lines().count())
    }

    fn run(&self, command: &str) -> Output {
        let flake = format!("{}#box", self.dir.join("flake").display());
        Command::new(env!("CARGO_BIN_EXE_pin-oci"))
            .args([command, "--flake", &flake])
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn lock_writes_the_lockfile_and_check_then_passes() {
    let fx = Fixture::new("round-trip");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();

    assert_eq!(
        fx.run("check").status.code(),
        Some(1),
        "unlocked before lock"
    );

    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let lock = fs::read_to_string(fx.lockfile()).unwrap();
    assert!(lock.contains("\"docker.io/library/caddy:2-alpine\""));
    assert!(lock.contains("\"docker.io/acme/solo:1\""));
    assert!(
        lock.contains("sha256:173b26306d711395accaeba8b67afcaad2a085ccb1e6bf26010bfe8c095a5229")
    );
    for system in [
        "x86_64-linux",
        "aarch64-linux",
        "armv7l-linux",
        "riscv64-linux",
    ] {
        assert!(lock.contains(&format!("\"{system}\"")), "{system}");
    }
    assert!(lock.ends_with("}\n"));

    let out = fx.run("check");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn second_lock_neither_calls_skopeo_nor_touches_the_file() {
    let fx = Fixture::new("idempotent");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();

    assert_eq!(fx.run("lock").status.code(), Some(0));
    let (before, calls) = (fs::read(fx.lockfile()).unwrap(), fx.skopeo_calls());
    assert_eq!(calls, 3);

    assert_eq!(fx.run("lock").status.code(), Some(0));
    assert_eq!(fx.skopeo_calls(), calls);
    assert_eq!(fs::read(fx.lockfile()).unwrap(), before);
}

#[test]
fn failure_is_all_or_nothing_and_names_the_culprit() {
    let fx = Fixture::new("all-or-nothing");
    fx.nix("x86_64-linux", &[CADDY, NOPE]);
    fx.skopeo();

    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(err.contains("docker.io/library/nope:1") && err.contains("manifest unknown"));
    assert!(!fx.lockfile().exists(), "no partial lockfile");

    // An existing lockfile survives a failed run byte for byte.
    fx.nix("x86_64-linux", &[SOLO]);
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let before = fs::read(fx.lockfile()).unwrap();
    fx.nix("x86_64-linux", &[SOLO, NOPE]);
    assert_eq!(fx.run("lock").status.code(), Some(2));
    assert_eq!(fs::read(fx.lockfile()).unwrap(), before);
}

#[test]
fn image_without_a_manifest_for_the_host_platform_fails() {
    let fx = Fixture::new("host-platform");
    fx.nix("i686-linux", &[CADDY]);
    fx.skopeo();

    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("host platform i686-linux"));
    assert!(!fx.lockfile().exists());
}

#[test]
fn stale_entries_are_pruned() {
    let fx = Fixture::new("prune");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));

    fx.nix("x86_64-linux", &[CADDY]);
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let lock = fs::read_to_string(fx.lockfile()).unwrap();
    assert!(lock.contains("caddy") && !lock.contains("acme/solo"));
}

#[test]
fn missing_skopeo_and_corrupt_lockfile_are_errors() {
    let fx = Fixture::new("errors");
    fx.nix("x86_64-linux", &[CADDY]);

    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("skopeo"));

    fx.skopeo();
    fs::write(fx.lockfile(), "{").unwrap();
    assert_eq!(fx.run("lock").status.code(), Some(2));
    assert_eq!(fs::read_to_string(fx.lockfile()).unwrap(), "{");
}
