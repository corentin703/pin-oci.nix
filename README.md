# pin-oci.nix

Deterministic pinning of container images for NixOS, on top of
`virtualisation.oci-containers`.

You declare images with floating tags (`caddy:2-alpine`). A small CLI, run outside
the Nix sandbox, resolves them to immutable per-platform digests and writes them to a
committed `pin-oci.lock`. The NixOS module reads that file and makes
`virtualisation.oci-containers` run `caddy@sha256:…`. Nix evaluation never touches the
network, and an image without a valid digest for the host platform fails evaluation
with the command to run.

> **Status.** Milestones 1 (pinning engine, `pin-oci lock` / `pin-oci check`) and 2
> (networks and volumes) are implemented. Compose-style projects (milestone 3) are
> specified in [`BIBLE.md`](BIBLE.md) but not implemented yet.

- [User manual](#user-manual)
- [Development environment](#development-environment)
- [Repository layout](#repository-layout)

---

## User manual

### 1. Install

Add the flake as an input. `follows` keeps the module aligned with your nixpkgs:

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    pin-oci = {
      url = "github:cprag/pin-oci.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { nixpkgs, pin-oci, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        pin-oci.nixosModules.default
        ./configuration.nix
      ];
    };
  };
}
```

The CLI is `pin-oci.packages.<system>.default`. Run it with `nix run github:cprag/pin-oci.nix -- <args>`
or add it to `environment.systemPackages` / your dev shell.

It needs `nix` and `skopeo` at run time. The packaged binary already carries `skopeo` in
its `PATH`.

### 2. Declare containers

`virtualisation.pin-oci.containers.<name>` accepts exactly the options of
`virtualisation.oci-containers.containers.<name>` (ports, volumes, environment,
`dependsOn`, …). Only `image` is special: write it with a floating tag.

```nix
{
  virtualisation.oci-containers.backend = "podman"; # or "docker"

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    containers.traefik = {
      image = "traefik:v3.0";
      ports = [ "80:80" "443:443" ];
      volumes = [ "/var/run/docker.sock:/var/run/docker.sock:ro" ];
    };
  };
}
```

`lockFile` has **no default** and is required when `enable = true`. Put the lockfile at the
flake root, next to `flake.lock`:

- `lockFile = ./pin-oci.lock;` if the configuration file is at the flake root;
- otherwise `lockFile = self + "/pin-oci.lock";`, which needs `self` passed with
  `specialArgs = { inherit self; };` in `nixosSystem`.

Under the hood each container is lowered to
`virtualisation.oci-containers.containers.<name>` with `image` replaced by
`<registry>/<repository>@sha256:<digest of your platform>`. The backend is whatever
`virtualisation.oci-containers.backend` says; pin-oci never starts containers itself.

### 3. Lock

From the flake root, resolve every image and write `pin-oci.lock`:

```console
$ pin-oci lock --flake .#myhost
pin-oci: wrote ./pin-oci.lock (1 image(s))
  + docker.io/library/traefik:v3.0
$ git add pin-oci.lock
```

> **`git add` is mandatory.** In a flake, Nix does not see files that git does not track.
> An untracked lockfile looks missing and the assertion below fires.

`--flake` takes `<flake>#<host>`, the name under `nixosConfigurations`. A reference without
`#<host>` is an error.

Behaviour worth knowing:

- **Only missing images are resolved.** An entry already in the lockfile for your platform
  is never re-resolved, so a moving tag (`latest`, `v3`) does not change under you.
  Running `pin-oci lock` on an up-to-date lockfile does nothing and does not touch the file.
- **To refresh an image**, delete its entry from `pin-oci.lock` (or the whole file) and run
  `pin-oci lock` again.
- **Stale entries are removed:** images no longer declared disappear from the lockfile.
- **All or nothing.** If any image fails to resolve, every failure is listed, the lockfile
  is left untouched and the exit code is 2.
- **Multi-platform.** All supported platforms of an index are recorded, so one lockfile
  serves `x86_64-linux` and `aarch64-linux` hosts. An image with no manifest for the host
  platform makes `lock` fail.
- **Deterministic output.** Sorted keys, fixed field order, atomic write (temporary file
  then rename).
- `--file <path>` overrides the lockfile location. It defaults to
  `<flake root>/pin-oci.lock` and is **required** for a remote flake reference
  (`github:…`), which has no local root.

Supported platforms (Linux only):

| OCI `os` / `architecture` | Nix system |
| --- | --- |
| `linux` / `amd64` | `x86_64-linux` |
| `linux` / `arm64` (`v8` or none) | `aarch64-linux` |
| `linux` / `arm` `v7` | `armv7l-linux` |
| `linux` / `riscv64` | `riscv64-linux` |

Other entries of an index (`s390x`, `ppc64le`, `arm` v5/v6, `windows`, `unknown/unknown`
attestations) are ignored. An image that is not an index and whose single platform is
unsupported is an error.

### 4. Check (CI)

```console
$ pin-oci check --flake .#myhost
pin-oci: 1 image(s) pinned for 'x86_64-linux' in ./pin-oci.lock
```

`check` never modifies anything and never contacts a registry. It verifies that every
declared image has an entry for the host platform.

| Exit code | Meaning |
| --- | --- |
| 0 | in sync |
| 1 | out of sync: missing images are listed with the command to run |
| 2 | error: `nix` failed, unreadable or invalid lockfile, remote flake without `--file` |

Entries in the lockfile that are no longer declared do not make `check` fail; only
`lock` prunes them.

### 5. Rebuild

`nixos-rebuild` and `nix build` on the configuration stay offline. When something is
not locked for your platform, evaluation stops with:

```text
[pin-oci] Missing or unpinned images for 'x86_64-linux' in /nix/store/…-source/pin-oci.lock:
  - docker.io/library/traefik:v3.0

Synchronize the lock file with the command:
  pin-oci lock
then make it visible to the flake:
  git add pin-oci.lock
```

`pin-oci lock` and `check` evaluate only the list of images and the host platform, never
the lockfile, so they work on a configuration that does not build yet.

### 6. Networks and volumes

Declare shared networks and volumes once; pin-oci creates them before the containers that
use them.

```nix
{
  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    networks.app-net = {
      subnet = "10.89.1.0/24"; # optional
      internal = false;        # optional, true = no Internet gateway
    };

    volumes.postgres_data = {
      driver = "local";        # optional, the default
    };

    containers.db = {
      image = "postgres:16-alpine";
      volumes = [ "postgres_data:/var/lib/postgresql/data" ];
      networks = [ "app-net" ];  # or extraOptions = [ "--network=app-net" ];
    };
  };
}
```

For each network and volume a `oneshot` unit `<backend>-network-<name>.service` /
`<backend>-volume-<name>.service` is generated. It runs `<backend> network|volume inspect`
and creates the resource only when it is absent, so running it again changes nothing, and a
resource removed by hand is recreated the next time the unit runs. The units are enabled at
boot, so declared resources exist even if no container uses them. With the `docker` backend
they are ordered after `docker.service`.

A container gets `after` and `requires` on every declared resource it references:

- a network, through `networks`, `extraOptions = [ "--network=<n>" ]`, `"--net=<n>"` or the
  two-element form `[ "--network" "<n>" ]` (a suffix such as `:alias=db` is ignored);
- a volume, through the source of a `volumes` entry (`"<name>:<path>[:opts]"`).

Names that are not declared in `virtualisation.pin-oci.networks` / `volumes` (`host`,
`bridge`, a bind mount like `/srv:/srv`, a volume you manage yourself) get no dependency and
are left to the backend. Network and volume names must match
`[a-zA-Z0-9][a-zA-Z0-9_.-]*`, otherwise evaluation fails.

> **Limitation.** Resources are created by root. A rootless Podman container
> (`podman.user` other than `root`) does not see them.

### Image references

References are normalized to a canonical key (the key of `pin-oci.lock`):

| You write | Key |
| --- | --- |
| `redis:7` | `docker.io/library/redis:7` |
| `redis` | `docker.io/library/redis:latest` |
| `prom/prometheus:v2.50.0` | `docker.io/prom/prometheus:v2.50.0` |
| `ghcr.io/a/b:1` | `ghcr.io/a/b:1` |
| `localhost:5000/foo` | `localhost:5000/foo:latest` |

`redis:7` and `docker.io/library/redis:7` share one entry. A reference that already
contains a digest (`redis@sha256:…`) is considered pinned: it is passed through unchanged
and has no entry in the lockfile. A malformed reference (`redis:`) fails evaluation.

### Registry access and authentication

`pin-oci` embeds no registry client. It runs `skopeo`, which reads your own credentials,
the same ones Docker and Podman use (`REGISTRY_AUTH_FILE`,
`$XDG_RUNTIME_DIR/containers/auth.json`, `~/.docker/config.json`), credential helpers
included. Log in with `docker login` / `podman login` / `skopeo login` as usual.

`pin-oci` passes `--registries-conf /dev/null` to `skopeo`: image references are always
fully qualified, so your host's `registries.conf` (search registries, mirrors) is ignored.
This avoids locking a stale tag from a mirror and avoids failures on a `registries.conf` in
the old v1 format. The consequence is that registries declared `insecure` (plain HTTP) in
that file are not usable.

### Troubleshooting

| Symptom | Cause / fix |
| --- | --- |
| Assertion `[pin-oci] Missing or unpinned images` right after `pin-oci lock` | The lockfile is not tracked by git: `git add pin-oci.lock`. |
| `The option … 'virtualisation.pin-oci.lockFile' was accessed but has no value defined` | Set `lockFile` (see [Declare containers](#2-declare-containers)). |
| `no host in flake reference` | Use `--flake .#<host>`. |
| `'github:…' is not a local flake: pass --file <path>` | Remote flake references need an explicit `--file`. |
| `cannot run skopeo` / `cannot run nix` | The binary is not in `PATH` (not needed with the packaged `pin-oci`, which wraps `skopeo`). |
| `skopeo failed … manifest unknown` | The tag does not exist in the registry. |
| `no manifest for the host platform …` | The image is not published for your architecture. |

---

## Development environment

### Requirements

- [Nix](https://nixos.org/download) with flakes enabled
  (`experimental-features = nix-command flakes`).
- [devenv](https://devenv.sh) (recommended) and [direnv](https://direnv.net) (optional).

### Enter the environment

With `devenv` (Rust toolchain from nixpkgs, plus `skopeo`):

```console
$ devenv shell
```

With `direnv`, the provided `.envrc` loads it automatically:

```console
$ direnv allow
```

Without `devenv`, the flake provides an equivalent shell for contributors:

```console
$ nix develop
```

Both give `cargo`, `rustc`, `clippy` and `skopeo`.

### Everyday commands

```console
$ cargo fmt --check
$ cargo clippy --all-targets -- -D warnings
$ cargo test                 # Rust unit + end-to-end tests
$ nix flake check            # builds the package (runs cargo test) + Nix module tests
$ nix build                  # the pin-oci binary, wrapped with skopeo
$ nix run . -- check --flake path/to/flake#host
```

`devenv.yaml` and `flake.nix` pin nixpkgs independently; update them with `devenv update`
and `nix flake update`.

### Tests

Every feature ships with its tests; all of them run offline and deterministically (no live
registry).

| Layer | Where | What |
| --- | --- | --- |
| Rust unit | `src/*.rs` (`#[cfg(test)]`) | lockfile model, platform mapping, manifest resolution, `check` / `lock` logic, with `nix` and `skopeo` behind traits and stubbed. |
| Rust end-to-end | `tests/check_cli.rs`, `tests/lock_cli.rs` | The real binary against a fake `nix` and a fake `skopeo` placed in `PATH`. `tests/fixtures/` holds real `skopeo` outputs captured once. |
| Nix modules | `tests/nix/default.nix`, `infra.nix` (run by `nix flake check`) | Reference normalization, module evaluation against fixture lockfiles (present / missing / wrong architecture / corrupt), the generated systemd units, laziness of `lockTargets`, container ordering, and the exact expression the CLI evaluates (`src/apply.nix`, shared with Rust). |
| Generated scripts | `tests/nix/infra-scripts.nix` | The network and volume unit scripts executed against a stateful fake backend: idempotence, flags, recreation, failure propagation. |
| NixOS VM | `tests/nix/vm-infra.nix` | Real `podman` and `docker` in a VM: resources created at boot, flags applied, units idempotent, container unit ordered after its resources. Needs KVM (`system-features = kvm nixos-test`). |

Run a single check, for instance:

```console
$ nix build .#checks.x86_64-linux.module-tests -L   # fast, evaluation only
$ nix build .#checks.x86_64-linux.infra-scripts -L
$ nix build .#checks.x86_64-linux.vm-infra -L        # boots two VMs, about a minute
```

When a test fails it prints the failing cases. To confirm a guarantee is really tested,
break the code and check that a test fails.

### Conventions

- [`BIBLE.md`](BIBLE.md) is the specification and wins over code. Change it first when the
  design changes.
- [`AGENTS.md`](AGENTS.md) (also `CLAUDE.md`) is the working protocol: one task at a time,
  tests in the same change, nothing `pub` by default, no network at evaluation time.
- Keep logic separate from I/O: `nix` and `skopeo` calls sit behind the `Evaluator` and
  `Runtime` traits so they can be stubbed.

---

## Repository layout

```text
pin-oci.nix/
├── src/
│   ├── main.rs         # CLI entry point, clap subcommands
│   ├── check.rs        # `check`: declared images vs lockfile
│   ├── lock.rs         # `lock`: resolve missing, prune, write
│   ├── nix.rs          # nix eval boundary, <flake>#<host> handling
│   ├── registry.rs     # skopeo boundary, index / single-manifest resolution
│   ├── lockfile.rs     # lockfile model, platform mapping, deterministic write
│   └── apply.nix       # expression the CLI evaluates on the configuration
├── modules/
│   ├── default.nix     # module entry point
│   ├── containers.nix  # milestone 1: options, assertion, lowering
│   ├── infra.nix       # milestone 2: network / volume units, container ordering
│   └── refs.nix        # pure image reference normalization
├── tests/              # Rust e2e + fixtures; nix/ holds the module, script and VM tests
├── flake.nix           # nixosModules.default, packages.pin-oci, devShell, checks
├── devenv.nix          # development environment
└── BIBLE.md            # specification
```
