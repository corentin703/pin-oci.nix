//! End-to-end: `pin-oci completions <shell>` (the real binary). It needs neither `nix` nor
//! `skopeo`: the environment has an empty `PATH`, so any attempt to run them would fail.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin-oci"))
        .arg("completions")
        .args(args)
        .env("PATH", "")
        .output()
        .unwrap()
}

#[test]
fn prints_a_script_for_each_supported_shell_and_nothing_else() {
    for shell in ["bash", "zsh", "fish", "elvish", "powershell"] {
        let out = run(&[shell]);
        assert_eq!(out.status.code(), Some(0), "{shell}");
        let script = String::from_utf8_lossy(&out.stdout);
        assert!(script.contains("pin-oci"), "{shell}: {script}");
        assert!(out.stderr.is_empty(), "{shell}: nothing on stderr");
    }
}

#[test]
fn the_script_follows_the_cli_modes() {
    let out = run(&["bash"]);
    let script = String::from_utf8_lossy(&out.stdout);
    for word in ["check", "update", "list"] {
        assert!(script.contains(word), "{word}");
    }
}

#[test]
fn unknown_or_missing_shell_is_rejected() {
    assert_eq!(run(&["tcsh"]).status.code(), Some(2));
    assert_eq!(run(&[]).status.code(), Some(2));
}
