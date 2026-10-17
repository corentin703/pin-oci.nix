//! End-to-end: `pin-oci convert` (the real binary) on compose files. The environment has an
//! empty `PATH`: conversion needs neither `nix`, `skopeo` nor the network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/compose");

fn fixture(name: &str) -> PathBuf {
    Path::new(FIXTURES).join(name)
}

fn expected(name: &str) -> String {
    fs::read_to_string(fixture(name)).unwrap()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pin-oci-convert-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn convert(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin-oci"))
        .arg("convert")
        .args(args)
        .env("PATH", "")
        .output()
        .unwrap()
}

fn on(file: &Path, extra: &[&str]) -> Output {
    let mut args = vec![file.to_str().unwrap()];
    args.extend_from_slice(extra);
    convert(&args)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn a_clean_file_converts_to_the_reference_module_without_a_word_on_stderr() {
    let out = on(&fixture("monitoring.yml"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), expected("monitoring.nix"));
    assert!(out.stderr.is_empty(), "{}", stderr(&out));
}

#[test]
fn unconverted_keys_are_reported_on_stderr_and_the_rest_is_converted() {
    let out = on(&fixture("shop.yml"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), expected("shop.nix"));
    let err = stderr(&out);
    assert!(
        err.contains("pin-oci: warning: services.cache.healthcheck: not supported, ignored"),
        "{err}"
    );
    assert!(
        err.contains("pin-oci: warning: services.cache.networks.back.aliases: not supported"),
        "{err}"
    );
    assert_eq!(err.lines().count(), 2, "{err}");
}

#[test]
fn the_output_is_deterministic() {
    let first = stdout(&on(&fixture("shop.yml"), &[]));
    assert_eq!(first, stdout(&on(&fixture("shop.yml"), &[])));
}

#[test]
fn strict_fails_on_any_warning_and_writes_nothing() {
    let out = on(&fixture("shop.yml"), &["--strict"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty(), "nothing on stdout: {}", stdout(&out));
    assert!(stderr(&out).contains("--strict: 2 warning(s), nothing written"));

    let dir = temp("strict-output");
    let target = dir.join("out.nix");
    let out = on(
        &fixture("shop.yml"),
        &["--strict", "--output", target.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!target.exists(), "--strict must not leave a file behind");
}

#[test]
fn strict_passes_a_file_without_warnings() {
    let out = on(&fixture("monitoring.yml"), &["--strict"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), expected("monitoring.nix"));
}

#[test]
fn output_writes_the_file_and_keeps_stdout_for_the_summary() {
    let dir = temp("output");
    let target = dir.join("shop.nix");
    let out = on(
        &fixture("shop.yml"),
        &["--output", target.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(fs::read_to_string(&target).unwrap(), expected("shop.nix"));
    assert!(stdout(&out).contains("wrote") && stdout(&out).contains("(2 warning(s))"));
    assert!(
        !stdout(&out).contains("virtualisation"),
        "the module is not echoed"
    );
}

#[test]
fn the_project_name_comes_from_the_flag_then_the_file_then_the_directory() {
    let dir = temp("naming").join("myproject");
    fs::create_dir_all(&dir).unwrap();
    let bare = dir.join("docker-compose.yml");
    fs::write(&bare, "services:\n  a:\n    image: caddy:2-alpine\n").unwrap();

    assert!(stdout(&on(&bare, &[])).contains("projects.\"myproject\""));
    assert!(stdout(&on(&bare, &["--name", "other"])).contains("projects.\"other\""));
    // The file's own `name:` beats the directory, the flag beats both.
    let named = on(&fixture("monitoring.yml"), &[]);
    assert!(stdout(&named).contains("projects.\"monitoring\""));
    let flagged = on(&fixture("monitoring.yml"), &["--name", "ops"]);
    assert!(stdout(&flagged).contains("projects.\"ops\""));
}

#[test]
fn interpolation_and_unquoted_numbers_are_errors_listing_the_values() {
    let dir = temp("unfaithful");
    let file = dir.join("c.yml");
    fs::write(
        &file,
        "name: x\nservices:\n  a:\n    image: nginx:${TAG}\n    environment:\n      V: 1.10\n",
    )
    .unwrap();
    let out = on(&file, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.contains("  - services.a.image"), "{err}");
    assert!(err.contains("  - services.a.environment.V"), "{err}");
}

#[test]
fn a_service_that_only_builds_is_an_error() {
    let dir = temp("build");
    let file = dir.join("c.yml");
    fs::write(&file, "name: x\nservices:\n  a:\n    build: .\n").unwrap();
    let out = on(&file, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("pin-oci only runs images"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn unreadable_and_invalid_files_are_errors_with_a_clear_message() {
    let out = convert(&["/nonexistent/compose.yml"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("cannot read"), "{}", stderr(&out));

    let dir = temp("invalid");
    let file = dir.join("c.yml");
    fs::write(&file, "services:\n  a:\n    image: [\n").unwrap();
    let out = on(&file, &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains("invalid YAML") && stderr(&out).contains("line 3"));

    assert_eq!(convert(&[]).status.code(), Some(2), "the file is required");
}
