//! Shared by the e2e tests that put fake executables on PATH.
//!
//! Writing an executable while another test thread forks makes the child inherit the write
//! descriptor, and the `exec` of that file then fails with ETXTBSY ("Text file busy").
//! Writes therefore exclude every spawn made by the test process.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::RwLock;

static SPAWN: RwLock<()> = RwLock::new(());

pub fn write_executable(path: &Path, content: &str) {
    let _exclusive = SPAWN.write().unwrap_or_else(|e| e.into_inner());
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

pub fn try_output(cmd: &mut Command) -> std::io::Result<Output> {
    let _shared = SPAWN.read().unwrap_or_else(|e| e.into_inner());
    cmd.output()
}

pub fn output(cmd: &mut Command) -> Output {
    try_output(cmd).unwrap()
}
