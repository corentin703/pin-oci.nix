# Changelog

Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions follow the
`version` of `Cargo.toml`.

## [Unreleased]

### Added
- `projects.<name>.followLockFile`: a project shipped by a flake can use the flake's own
  `pin-oci.lock` for its images; the host can override it. Without it nothing changes, and the
  CLI and the lockfile format are untouched.

## [0.2.0] - 2026-11-07

### Added
- `pin-oci lock` writes an empty lockfile (`{"version": 1, "images": {}}`) when nothing is
  declared, like `nix flake lock` rewrites the lockfile to match the flake: existing entries
  are pruned.
- `pin-oci lock update [image...]` moves locked tags to their current digests; `--dry` shows what
  would move without writing the lockfile.
- `pin-oci lock list` shows declared and locked images with their status.
- `pin-oci completions <shell>`; the Nix package installs the bash, zsh and fish scripts.
- `pin-oci convert` turns a docker-compose file into a project module.
- `lock` without a host aggregates every `nixosConfigurations` host that enables pin-oci.
- `pullImage`: locked images can be fetched into the Nix store, `nixHash` is computed by `lock`.
- Compose-style `projects`, `networks` and `volumes` compiled to `oci-containers`, with a
  `<backend>-<project>.target` per project.
- Opt-in overlay exposing `pkgs.pin-oci`; Cachix binary cache declared in the flake.

### Changed
- **Breaking:** `check`, `--update` and `--list` are now `lock check`, `lock update` and
  `lock list`. The old flags are usage errors.
- **Breaking:** a flake where no host enables pin-oci is no longer an error (exit 2). `lock`,
  `lock check` and `lock list` succeed on an empty declaration and print
  `warning: no host uses pin-oci, nothing is declared`. A failing `nix eval` is still an error.
- Dependencies: `annotate-snippets` 0.12.17, `encoding_rs` 0.8.44, `granit-parser` 2.1.0,
  `serde-saphyr` 2.1.1, `smallvec` 1.16.5, `unicode-width` 0.2.3; `nixpkgs` and `devenv` locks
  updated. `cargo audit` reports no advisory.

### Fixed
- A store-path volume source no longer breaks evaluation.

## [0.1.0]

Initial version: `lock` and `check` with skopeo-based resolution, the `virtualisation.pin-oci`
module with fail-fast assertion on unlocked images.
