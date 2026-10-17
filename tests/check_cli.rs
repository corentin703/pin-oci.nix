//! End-to-end: `pin-oci lock --check` (the real binary) against a fake `nix` on PATH. Offline.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const KEY: &str = "docker.io/library/caddy:2-alpine";
const HEX: &str = "d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499d8a26e8499abcd";

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pin-oci-e2e-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("flake")).unwrap();
        Self { dir }
    }

    fn flake(&self) -> PathBuf {
        self.dir.join("flake")
    }

    fn fake_nix(&self, body: &str) {
        let path = self.dir.join("bin/nix");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Fake `nix` that records its arguments and prints `json`.
    fn nix_printing(&self, json: &str) {
        let args = self.dir.join("args");
        self.fake_nix(&format!(
            "printf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{json}'",
            args.display()
        ));
    }

    fn lockfile(&self, system: &str) {
        let json = format!(
            r#"{{"version":1,"images":{{"{KEY}":{{"registry":"docker.io","repository":"library/caddy","tag":"2-alpine","arch":{{"{system}":{{"digest":"sha256:{HEX}","os":"linux","architecture":"amd64"}}}}}}}}}}"#
        );
        fs::write(self.flake().join("pin-oci.lock"), json).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_pin-oci"))
            .args(["lock", "--check"])
            .args(args)
            .env("PATH", self.dir.join("bin"))
            .output()
            .unwrap()
    }

    fn run_host(&self) -> Output {
        self.run(&["--flake", &format!("{}#box", self.flake().display())])
    }
}

fn declared(targets: &[&str]) -> String {
    let list: Vec<String> = targets
        .iter()
        .map(|t| format!(r#""{t}":{{"registry":"r","repository":"p","tag":"t"}}"#))
        .collect();
    format!(
        r#"{{"system":"x86_64-linux","targets":{{{}}}}}"#,
        list.join(",")
    )
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

#[test]
fn exit_0_when_in_sync_and_nix_is_queried_for_lock_targets() {
    let fx = Fixture::new("sync");
    fx.nix_printing(&declared(&[KEY]));
    fx.lockfile("x86_64-linux");

    let out = fx.run_host();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    let args = read(&fx.dir.join("args"));
    assert!(args.contains("--json"));
    assert!(args.contains("#nixosConfigurations.\"box\".config"));
    assert!(args.contains("lockTargets"));
}

#[test]
fn exit_1_and_lock_command_when_an_image_is_missing() {
    let fx = Fixture::new("missing");
    fx.nix_printing(&declared(&[KEY, "docker.io/library/redis:7"]));
    fx.lockfile("x86_64-linux");

    let out = fx.run_host();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("docker.io/library/redis:7") && err.contains("pin-oci lock"));
}

#[test]
fn exit_1_when_the_host_architecture_is_not_locked() {
    let fx = Fixture::new("arch");
    fx.nix_printing(&declared(&[KEY]));
    fx.lockfile("aarch64-linux");

    assert_eq!(fx.run_host().status.code(), Some(1));
}

#[test]
fn exit_1_when_the_lockfile_does_not_exist() {
    let fx = Fixture::new("no-lock");
    fx.nix_printing(&declared(&[KEY]));

    assert_eq!(fx.run_host().status.code(), Some(1));
}

#[test]
fn exit_2_when_nix_fails() {
    let fx = Fixture::new("nix-fails");
    fx.fake_nix("echo boom >&2\nexit 1");
    fx.lockfile("x86_64-linux");

    let out = fx.run_host();
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("boom"));
}

#[test]
fn exit_2_when_nix_is_not_installed() {
    let fx = Fixture::new("no-nix");
    fx.lockfile("x86_64-linux");

    assert_eq!(fx.run_host().status.code(), Some(2));
}

#[test]
fn exit_2_on_corrupt_lockfile() {
    let fx = Fixture::new("corrupt");
    fx.nix_printing(&declared(&[KEY]));
    fs::write(fx.flake().join("pin-oci.lock"), "{").unwrap();

    assert_eq!(fx.run_host().status.code(), Some(2));
}

#[test]
fn exit_2_for_remote_flake_without_file() {
    let fx = Fixture::new("remote");
    fx.nix_printing(&declared(&[KEY]));

    let out = fx.run(&["--flake", "github:owner/repo#box"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("--file"));
}

#[test]
fn explicit_file_is_used_for_a_remote_flake() {
    let fx = Fixture::new("remote-file");
    fx.nix_printing(&declared(&[KEY]));
    fx.lockfile("x86_64-linux");
    let file = fx.flake().join("pin-oci.lock");

    let out = fx.run(&[
        "--flake",
        "github:owner/repo#box",
        "--file",
        file.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
}

#[test]
fn the_old_check_subcommand_is_rejected() {
    let fx = Fixture::new("old-check");
    fx.nix_printing(&declared(&[KEY]));

    let out = Command::new(env!("CARGO_BIN_EXE_pin-oci"))
        .args(["check", "--flake", &format!("{}#box", fx.flake().display())])
        .env("PATH", fx.dir.join("bin"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("unrecognized subcommand"),
        "{}",
        stderr(&out)
    );
}
