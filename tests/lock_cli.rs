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

const AMD64: &str = "sha256:173b26306d711395accaeba8b67afcaad2a085ccb1e6bf26010bfe8c095a5229";
const NEW_AMD64: &str = "sha256:eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

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

    /// Like `nix`, but the declared images are pulled into the store, and `nix hash file`
    /// answers with `hash` (the sha256 of the file it is given, in the real binary).
    fn nix_pulling(&self, system: &str, targets: &[&str], hash: &str) {
        let targets: Vec<String> = targets
            .iter()
            .map(|t| t.replace('}', r#","pullImage":true}"#))
            .collect();
        let json = format!(
            r#"{{"system":"{system}","targets":{{{}}}}}"#,
            targets.join(",")
        );
        let log = self.dir.join("nix.log");
        self.executable(
            "nix",
            &format!(
                r#"echo "$*" >> '{log}'
case "$1" in
  hash) [ -f "$6" ] || {{ echo "no archive to hash: $6" >&2; exit 1; }}; echo '{hash}' ;;
  *) printf '%s' '{json}' ;;
esac"#,
                log = log.display()
            ),
        );
    }

    fn nix_log(&self) -> String {
        fs::read_to_string(self.dir.join("nix.log")).unwrap_or_default()
    }

    fn skopeo(&self) {
        self.skopeo_serving(&format!("{FIXTURES}/skopeo-index.json"));
    }

    /// Fake skopeo whose caddy index is read from `caddy_index` (to simulate a moved tag).
    fn skopeo_serving(&self, caddy_index: &str) {
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
  *" copy "*) for last; do :; done; dest="${{last#docker-archive://}}"; dest="${{dest%%:*}}"; echo archive > "$dest" ;;
  *--raw*/library/caddy:*) emit '{caddy}' ;;
  *--raw*/acme/solo:*) emit '{f}/skopeo-single-raw.json' ;;
  *--no-tags*/acme/solo:*) emit '{f}/skopeo-single-inspect.json' ;;
  *) echo "manifest unknown" >&2; exit 2 ;;
esac"#,
                log = log.display(),
                caddy = caddy_index,
                f = FIXTURES
            ),
        );
    }

    fn skopeo_calls(&self) -> usize {
        fs::read_to_string(self.dir.join("skopeo.log")).map_or(0, |log| log.lines().count())
    }

    fn lock_with(&self, extra: &[&str]) -> Output {
        let flake = format!("{}#box", self.dir.join("flake").display());
        Command::new(env!("CARGO_BIN_EXE_pin-oci"))
            .args(["lock", "--flake", &flake])
            .args(extra)
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
    }

    /// The caddy tag now points at other digests.
    fn move_caddy_tag(&self) {
        let index = fs::read_to_string(format!("{FIXTURES}/skopeo-index.json")).unwrap();
        let moved = index.replace(AMD64, NEW_AMD64);
        let path = self.dir.join("moved-index.json");
        fs::write(&path, moved).unwrap();
        self.skopeo_serving(path.to_str().unwrap());
    }

    fn skopeo_log(&self) -> String {
        fs::read_to_string(self.dir.join("skopeo.log")).unwrap_or_default()
    }

    fn check(&self) -> Output {
        let flake = format!("{}#box", self.dir.join("flake").display());
        Command::new(env!("CARGO_BIN_EXE_pin-oci"))
            .args(["lock", "check", "--flake", &flake])
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
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

    assert_eq!(fx.check().status.code(), Some(1), "unlocked before lock");

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

    let out = fx.check();
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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn update_follows_a_moved_tag_while_plain_lock_does_not() {
    let fx = Fixture::new("update-moved");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));

    fx.move_caddy_tag();
    let calls = fx.skopeo_calls();
    assert_eq!(fx.run("lock").status.code(), Some(0));
    assert_eq!(
        fx.skopeo_calls(),
        calls,
        "plain lock does not look at the registry"
    );
    // The solo fixture happens to report the same amd64 digest: count, do not just look.
    let amd64_before = fs::read_to_string(fx.lockfile())
        .unwrap()
        .matches(AMD64)
        .count();
    assert_eq!(amd64_before, 2);

    let out = fx.lock_with(&["update"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("~ docker.io/library/caddy:2-alpine"),
        "{}",
        stdout(&out)
    );
    assert!(
        stdout(&out).contains("-> sha256:eeeeeeeeeeee…"),
        "{}",
        stdout(&out)
    );
    let lock = fs::read_to_string(fx.lockfile()).unwrap();
    assert!(lock.contains(NEW_AMD64));
    assert_eq!(
        lock.matches(AMD64).count(),
        amd64_before - 1,
        "only caddy moved"
    );
    assert_eq!(fx.check().status.code(), Some(0));
}

#[test]
fn update_with_a_key_only_looks_up_that_image() {
    let fx = Fixture::new("update-key");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let caddy_lookups = fx.skopeo_log().matches("library/caddy").count();

    fx.move_caddy_tag();
    let out = fx.lock_with(&["update", "docker.io/acme/solo:1"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        fx.skopeo_log().matches("library/caddy").count(),
        caddy_lookups
    );
    assert!(!fs::read_to_string(fx.lockfile())
        .unwrap()
        .contains(NEW_AMD64));
}

#[test]
fn update_with_an_unknown_key_exits_2_without_calling_skopeo() {
    let fx = Fixture::new("update-unknown");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let (before, calls) = (fs::read(fx.lockfile()).unwrap(), fx.skopeo_calls());

    let out = fx.lock_with(&["update", "caddy:2-alpine"]);
    assert_eq!(out.status.code(), Some(2));
    let err = stderr(&out);
    assert!(
        err.contains("not a declared image: caddy:2-alpine"),
        "{err}"
    );
    assert!(
        err.contains("  - docker.io/library/caddy:2-alpine"),
        "{err}"
    );
    assert_eq!(fx.skopeo_calls(), calls);
    assert_eq!(fs::read(fx.lockfile()).unwrap(), before);
}

#[test]
fn a_failing_update_leaves_the_lockfile_byte_identical() {
    let fx = Fixture::new("update-fails");
    fx.nix("x86_64-linux", &[CADDY, SOLO]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let before = fs::read(fx.lockfile()).unwrap();

    // The registry now refuses caddy.
    fx.skopeo_serving("/nonexistent/index.json");
    let out = fx.lock_with(&["update"]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(fs::read(fx.lockfile()).unwrap(), before);
}

impl Fixture {
    /// Fake nix for a flake with hosts `pc` (x86_64) and `arm` (aarch64), both declaring caddy,
    /// and `plain`, which does not use pin-oci. `fail` makes `arm` fail to evaluate.
    fn nix_hosts(&self, fail: bool) {
        let arm = if fail {
            "echo 'error: boom on arm' >&2; exit 1".to_owned()
        } else {
            format!(r#"printf '%s' '{{"system":"aarch64-linux","targets":{{{CADDY}}}}}'"#)
        };
        self.executable(
            "nix",
            &format!(
                r#"case "$3" in
  *'#nixosConfigurations') printf '%s' '["arm","pc","plain"]' ;;
  *'"pc".config') printf '%s' '{{"system":"x86_64-linux","targets":{{{CADDY}}}}}' ;;
  *'"arm".config') {arm} ;;
  *'"plain".config') printf null ;;
  *) echo "unexpected: $*" >&2; exit 1 ;;
esac"#
            ),
        );
    }

    /// Runs in the flake directory with no `--flake`.
    fn bare(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pin-oci"))
            .arg("lock")
            .args(args)
            .current_dir(self.dir.join("flake"))
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
    }
}

#[test]
fn without_flake_every_host_of_the_flake_goes_into_one_lockfile() {
    let fx = Fixture::new("all-hosts");
    fx.nix_hosts(false);
    fx.skopeo();

    assert_eq!(fx.bare(&["check"]).status.code(), Some(1));

    let out = fx.bare(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(stdout(&out).contains("ignored, pin-oci not enabled: plain"));
    let lock = fs::read_to_string(fx.lockfile()).unwrap();
    assert!(lock.contains("aarch64-linux") && lock.contains("x86_64-linux"));
    assert!(
        !lock.contains("\"pc\"") && !lock.contains("plain"),
        "no host in the lockfile"
    );

    assert_eq!(fx.bare(&["check"]).status.code(), Some(0));
    let list = stdout(&fx.bare(&["list"]));
    assert!(list.contains("HOSTS") && list.contains("arm,pc"), "{list}");
    assert!(list.contains("aarch64-linux sha256:") && list.contains("x86_64-linux sha256:"));
}

#[test]
fn without_flake_one_failing_host_exits_2_and_writes_nothing() {
    let fx = Fixture::new("all-hosts-fail");
    fx.nix_hosts(true);
    fx.skopeo();

    let out = fx.bare(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("arm: ") && stderr(&out).contains("boom on arm"));
    assert!(!fx.lockfile().exists());
    assert_eq!(
        fx.skopeo_calls(),
        0,
        "nothing is resolved before every host evaluated"
    );
}

#[test]
fn without_flake_and_without_any_pin_oci_host_exits_2() {
    let fx = Fixture::new("no-host");
    fx.executable(
        "nix",
        r#"case "$3" in
  *'#nixosConfigurations') printf '%s' '["plain"]' ;;
  *) printf null ;;
esac"#,
    );
    let out = fx.bare(&[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("uses pin-oci"), "{}", stderr(&out));
}

const SRI: &str = "sha256-AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";

#[test]
fn lock_hashes_an_image_to_pull_with_the_command_pull_image_runs_then_check_passes() {
    let fx = Fixture::new("pull");
    fx.nix_pulling("x86_64-linux", &[CADDY, SOLO], SRI);
    fx.skopeo();

    assert_eq!(fx.check().status.code(), Some(1), "unlocked before lock");
    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let lock = fs::read_to_string(fx.lockfile()).unwrap();
    assert_eq!(lock.matches("nixHash").count(), 2, "{lock}");
    assert!(lock.contains(SRI));
    let log = fx.skopeo_log();
    let copy = log
        .lines()
        .find(|l| l.contains(" copy ") && l.contains("library/caddy"))
        .unwrap_or_else(|| panic!("no skopeo copy in:\n{log}"));
    assert!(
        copy.contains("--override-os linux --override-arch amd64 copy")
            && copy.contains(&format!("docker://docker.io/library/caddy@{AMD64}"))
            && copy.ends_with(":docker.io/library/caddy:2-alpine")
            && copy.contains("docker-archive://"),
        "{copy}"
    );
    assert!(fx.nix_log().contains("hash file --type sha256 --sri"));
    assert_eq!(fx.check().status.code(), Some(0));

    // Locked and hashed: a second run fetches nothing.
    let calls = fx.skopeo_calls();
    let again = fx.run("lock");
    assert_eq!(again.status.code(), Some(0));
    assert_eq!(fx.skopeo_calls(), calls);
}

#[test]
fn a_failing_hash_leaves_the_lockfile_absent_and_reports_the_image() {
    let fx = Fixture::new("pull-fail");
    fx.nix_pulling("x86_64-linux", &[CADDY], "not-a-hash");
    fx.skopeo();

    let out = fx.run("lock");
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    assert!(stderr(&out).contains("docker.io/library/caddy:2-alpine"));
    assert!(stderr(&out).contains("nix hash"), "{}", stderr(&out));
    assert!(!fx.lockfile().exists());
}

#[test]
fn check_reports_a_locked_image_to_pull_that_has_no_hash() {
    let fx = Fixture::new("pull-check");
    fx.nix("x86_64-linux", &[CADDY]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));

    fx.nix_pulling("x86_64-linux", &[CADDY], SRI);
    let out = fx.check();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("no nixHash")
            || stderr(&out).contains("no nixHash")
    );
}

#[test]
fn update_dry_shows_the_moved_digests_and_leaves_the_lockfile_byte_identical() {
    let fx = Fixture::new("update-dry");
    fx.nix("x86_64-linux", &[CADDY]);
    fx.skopeo();
    assert_eq!(fx.run("lock").status.code(), Some(0));
    let before = fs::read(fx.lockfile()).unwrap();

    fx.move_caddy_tag();
    let out = fx.lock_with(&["update", "--dry"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(stdout.contains("dry run, nothing written"), "{stdout}");
    assert!(
        stdout.contains("sha256:173b26306d71… -> sha256:eeeeeeeeeeee…"),
        "{stdout}"
    );
    assert_eq!(fs::read(fx.lockfile()).unwrap(), before);

    // The same command without --dry applies exactly that.
    assert_eq!(fx.lock_with(&["update"]).status.code(), Some(0));
    assert!(fs::read_to_string(fx.lockfile())
        .unwrap()
        .contains(NEW_AMD64));
}

#[test]
fn update_dry_does_not_hash_images_to_pull() {
    let fx = Fixture::new("update-dry-pull");
    fx.nix_pulling("x86_64-linux", &[CADDY], SRI);
    fx.skopeo();

    let out = fx.lock_with(&["update", "--dry"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(!fx.skopeo_log().contains(" copy "), "no image fetched");
    assert!(!fx.nix_log().contains("hash file"));
    assert!(!fx.lockfile().exists());
}
