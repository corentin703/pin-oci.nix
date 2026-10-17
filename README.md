# pin-oci.nix

Deterministic pinning of container images for NixOS, on top of
`virtualisation.oci-containers`.

You declare images with floating tags (`caddy:2-alpine`). A small CLI, run outside
the Nix sandbox, resolves them to immutable per-platform digests and writes them to a
committed `pin-oci.lock`. The NixOS module reads that file and makes
`virtualisation.oci-containers` run `caddy@sha256:…`. Nix evaluation never touches the
network, and an image without a valid digest for the host platform fails evaluation
with the command to run.

> **Status.** All three milestones of [`BIBLE.md`](BIBLE.md) are implemented: the pinning
> engine (`pin-oci lock` / `pin-oci lock --check`), shared networks and volumes, and compose-style
> projects.

- [User manual](#user-manual)
- [Development environment](#development-environment)
- [Repository layout](#repository-layout)

---

## User manual

### 1. Install

Add the flake as an input:

```nix
{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

    pin-oci.url = "github:corentin703/pin-oci.nix";
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

There is no `inputs.nixpkgs.follows`, on purpose. The module takes its options and `pkgs`
from your host configuration, never from the flake's own nixpkgs, so `follows` would not
align anything there. It would only make Nix build the CLI against your nixpkgs, which
differs from what CI built and defeats the [binary cache](#binary-cache-optional).

The CLI is `pin-oci.packages.<system>.default`. Run it with `nix run github:corentin703/pin-oci.nix -- <args>`
or add it to `environment.systemPackages` / your dev shell.

To write `pkgs.pin-oci` instead, opt in to the overlay. Nothing is added to your `pkgs`
unless you do:

```nix
{ pkgs, pin-oci, ... }:
{
  nixpkgs.overlays = [ pin-oci.overlays.default ];
  environment.systemPackages = [ pkgs.pin-oci ];
}
```

`pin-oci` must reach the module, for instance with `specialArgs = { inherit pin-oci; };` in
`nixosSystem`. The overlay only adds the `pin-oci` attribute, and it is the very derivation of
`packages.<system>.default`, so the binary cache below still applies.

It needs `nix` and `skopeo` at run time. The packaged binary already carries `skopeo` in
its `PATH`.

#### Shell completion

The package installs completion scripts for bash, zsh and fish, so they are available as
soon as `pin-oci` is in `environment.systemPackages` (enable `programs.bash.completion`,
or `programs.zsh.enable` / `programs.fish.enable`, as usual on NixOS). With `nix run`, or for
another shell, print the script yourself:

```console
$ pin-oci completions bash   # also zsh, fish, elvish, powershell
$ source <(pin-oci completions bash)
```

#### Binary cache (optional)

CI builds `pin-oci` and pushes it to a public [Cachix](https://pin-oci.cachix.org) cache.
Without the cache, Nix builds the CLI from source (Rust toolchain included). Declare it once
in your NixOS configuration, before the first rebuild that uses the package:

```nix
{
  nix.settings = {
    extra-substituters = [ "https://pin-oci.cachix.org" ];
    extra-trusted-public-keys = [
      "pin-oci.cachix.org-1:rnJNoJhMLyEkTckqap7U3sTpic1xvACQlpSNqFHTPys="
    ];
  };
}
```

Use the `extra-` forms: they add to `cache.nixos.org` instead of replacing it.

- **Not on NixOS** (Nix on another distribution or macOS): put the same two lines in
  `/etc/nix/nix.conf`, without the `nix.settings` wrapper, then restart the daemon.
  ```text
  extra-substituters = https://pin-oci.cachix.org
  extra-trusted-public-keys = pin-oci.cachix.org-1:rnJNoJhMLyEkTckqap7U3sTpic1xvACQlpSNqFHTPys=
  ```
- **One-off, without touching your configuration:**
  `nix run --accept-flake-config github:corentin703/pin-oci.nix -- <args>`. The flake carries the
  cache in its `nixConfig`, and `--accept-flake-config` trusts it for that command only.
  A flake input's `nixConfig` is ignored when you evaluate your own system, hence the
  `nix.settings` block above.
- **Check that it is used:** `nix build github:corentin703/pin-oci.nix --dry-run` lists the paths
  it would fetch (`will be fetched from 'https://pin-oci.cachix.org'`) instead of built.
  If it says `will be built`, the cache is not configured or not trusted, or the revision
  was not pushed yet.

The cache only holds what CI built on `x86_64-linux`. An `aarch64-linux` host builds the CLI
itself.

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

`--flake` defaults to `.`. With `<flake>#<host>` (a name under `nixosConfigurations`) only that
host is evaluated. Without `#<host>`, every host of the flake that enables pin-oci is evaluated and
`pin-oci.lock` holds the union of their images (the lockfile knows platforms, not hosts). Hosts
without pin-oci are ignored and named in the output. If any host fails to evaluate, nothing is
written (exit 2); it is also an error when no host enables pin-oci.

Behaviour worth knowing:

- **A plain `lock` only resolves missing images.** An entry already in the lockfile for your
  platform is never re-resolved (unless you pass `--update`), so a moving tag (`latest`, `v3`) does not change under you.
  Running `pin-oci lock` on an up-to-date lockfile does nothing and does not touch the file.
- **To refresh images**, use `--update`: it moves each tag to its current digests and
  rewrites the entry for every platform.

  ```console
  $ pin-oci lock --update --flake .#myhost                                   # every image
  $ pin-oci lock --update --flake .#myhost docker.io/library/caddy:2-alpine  # one image
  pin-oci: wrote ./pin-oci.lock (2 image(s))
    ~ docker.io/library/caddy:2-alpine
        x86_64-linux sha256:173b26306d71… -> sha256:eeeeeeeeeeee…
    = 1 image(s) already current
  ```

  Name an image by its **canonical key** (the key in `pin-oci.lock`, see
  [Image references](#image-references)): `caddy:2-alpine` is refused, and the error lists
  the declared keys. Nothing is written if any image fails. `--update` cannot be combined
  with `--check`.
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
$ pin-oci lock --check --flake .#myhost
pin-oci: 1 image(s) pinned for 'x86_64-linux' in ./pin-oci.lock
```

`lock --check` never modifies anything and never contacts a registry. It verifies that every
declared image has an entry for the host platform.

| Exit code | Meaning |
| --- | --- |
| 0 | in sync |
| 1 | out of sync: missing images are listed with the command to run |
| 2 | error: `nix` failed, unreadable or invalid lockfile, remote flake without `--file` |

Entries in the lockfile that are no longer declared do not make `--check` fail; only
a plain `lock` prunes them.

### 5. List

```console
$ pin-oci lock --list
pin-oci: images for 'aarch64-linux, x86_64-linux' in ./pin-oci.lock
  STATUS   IMAGE                             HOSTS    DIGEST
  missing  docker.io/acme/solo:1             pc
  locked   docker.io/library/caddy:2-alpine  arm,pc   aarch64-linux sha256:7c1e…
                                                      x86_64-linux sha256:173b26306d711395accaeba8b67afcaad2a085ccb1e6bf26010bfe8c095a5229
  stale    docker.io/old/thing:3             -        x86_64-linux sha256:0000000000000000000000000000000000000000000000000000000000000000
```

Every image that is declared or locked, sorted, with the hosts declaring it and its status for the
**platforms of those hosts**:

| Status | Meaning |
| --- | --- |
| `locked` | declared, and the lockfile has a digest for each of those platforms (shown in full) |
| `missing` | declared, but nothing is locked for at least one of them: run `pin-oci lock` |
| `stale` | in the lockfile but no longer declared: the next plain `lock` removes it |

`--list` only reads: it never writes the lockfile and never contacts a registry. It exits 0
whatever the statuses; use `--check` to fail a CI job. It cannot be combined with `--check`
or `--update`.

### 6. Rebuild

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

`pin-oci lock` and `lock --check` evaluate only the list of images and the host platform, never
the lockfile, so they work on a configuration that does not build yet.

### 7. Networks and volumes

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

### 8. Projects

A project groups interdependent services under one namespace, like a compose file. It is
sugar: it compiles into the containers, networks and volumes of the previous sections, so
locking, fail-fast and unit ordering apply unchanged.

```nix
{
  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    projects.monitoring = {
      enable = true;
      networks.default.enable = true;   # creates monitoring-default, every service joins it

      services = {
        prometheus = {
          image = "prom/prometheus:v2.50.0";
          volumes = [ "prom_data:/prometheus" ];
          ports = [ "9090:9090" ];
        };
        grafana = {
          image = "grafana/grafana:10.4.0";
          dependsOn = [ "prometheus" ];
          ports = [ "3000:3000" ];
          volumes = [ "grafana_data:/var/lib/grafana" ];
        };
      };

      volumes.prom_data = { };
      volumes.grafana_data = { };
    };
  };
}
```

A service accepts the options of a container. What the project does for you:

| You write | You get |
| --- | --- |
| service `grafana` in project `monitoring` | container `monitoring-grafana`, unit `<backend>-monitoring-grafana.service` |
| volume `grafana_data` | volume `monitoring_grafana_data`, created by its own unit |
| network `backend` | network `monitoring-backend`, created by its own unit |
| `volumes = [ "grafana_data:/x" ]` | rewritten to `monitoring_grafana_data:/x` |
| `networks = [ "backend" ]`, `extraOptions = [ "--network=backend" ]` | rewritten to `monitoring-backend` (a suffix like `:alias=db` is kept) |
| `dependsOn = [ "prometheus" ]` | `After` and `Requires` on `monitoring-prometheus` |
| `networks.default.enable = true` | every service joins `monitoring-default` |
| project `monitoring` | target `<backend>-monitoring.target`, grouping all its services |

**Project target.** `<backend>-<project>.target` (e.g. `podman-monitoring.target`) is started at
boot and `wants` every service unit of the project, which are `partOf` it. It runs nothing
itself; it lets you act on the whole project:

```console
$ systemctl restart podman-monitoring.target   # restarts every service of the project
$ systemctl stop podman-monitoring.target
```

Network and volume init units are not part of the target (they are `oneshot` units with
`RemainAfterExit`, required by the services that use them). A disabled project has no target.

**Names inside the project.** Containers are named `<project>-<service>`, so on its own
`prometheus` would not resolve. Each service therefore joins the default network with the
alias `<service>` (`monitoring-default:alias=prometheus` with Podman,
`name=monitoring-default,alias=prometheus` with Docker): `grafana` reaches `prometheus` by
that name. Other project networks add no automatic alias; write one in the entry if you
need it (`networks = [ "backend:alias=db" ]`, in the syntax of your backend).

**`external`.** `volumes.<v>.external = true` and `networks.<n>.external = true` refer to a
resource declared in the global `virtualisation.pin-oci.volumes` / `networks`: no prefix,
no unit, references left as written. Evaluation fails if the global resource does not
exist. The `default` network cannot be external.

```nix
virtualisation.pin-oci.volumes.shared_data = { };
virtualisation.pin-oci.projects.monitoring.volumes.shared_data.external = true;
```

Project, service, network and volume names must match `[a-zA-Z0-9][a-zA-Z0-9_.-]*`. Two
projects that generate the same name (`a-b` + service `c`, and `a` + service `b-c`) fail
evaluation, as does a `dependsOn` naming a service that is not in the project.

`pin-oci lock` locks project images with the others; the CLI needs no flag for projects.

### 9. Convert a compose file

`pin-oci convert` turns a `docker-compose.yml` into a project module, so you do not retype it:

```console
$ pin-oci convert docker-compose.yml > shop.nix
pin-oci: warning: services.cache.healthcheck: not supported, ignored
pin-oci: warning: services.cache.networks.back.aliases: not supported, ignored
$ pin-oci convert docker-compose.yml --output shop.nix   # write the file instead
```

Add `shop.nix` to the `imports` of your configuration, then lock the images (they stay
floating tags until you do): `pin-oci lock --flake .#myhost`. This is a **useful subset of
compose, not compose**: review the output.

| Compose | Becomes |
| --- | --- |
| `image`, `user`, `working_dir` | `image`, `user`, `workdir` |
| `command`, `entrypoint` (list, or a string split like a shell) | `cmd`, `entrypoint` (first word; the rest lead `cmd`) |
| `environment`, `labels` (map or `KEY=value` list) | `environment`, `labels` |
| `ports`, `volumes` (short and long syntax) | `ports`, `volumes` (named volumes are prefixed by the project, binds are not) |
| `depends_on` (list or map) | `dependsOn` (start order only) |
| `networks`, top-level `networks` (`external`, `internal`, first `ipam` subnet) | `networks`, project networks |
| top-level `volumes` (`external`, `driver`) | project volumes |
| `name:` | the project name (else `--name`, else the directory name) |

- **Nothing is dropped silently.** Each key that is not converted is a warning on stderr:
  `build`, `healthcheck`, `restart`, `env_file`, `deploy`, `secrets`, `container_name`,
  `depends_on` conditions other than `service_started`, service network aliases, tmpfs
  mounts, relative bind mounts (`./data` is resolved by the backend at run time, not
  relative to the file)... `--strict` makes any warning an error (exit 1, nothing written).
- **A value that cannot be kept as written is an error** (exit 2), listing every path:
  `${VAR}` interpolation (write the value in the file; `$$` is a literal `$`) and unquoted
  numbers that YAML rewrites (`1.10` reads as `1.1`, `007` as `7`: quote them). A service
  with only `build:` is an error: pin-oci runs images.
- **YAML as compose reads it:** only `true` and `false` are booleans (`DEBUG: no` stays the
  text `no`), anchors and `<<:` merge keys are resolved. Integers written in another base
  (`0x10`, `1_000`) are read by the YAML parser before pin-oci sees them, so they are not
  detected: quote such values.
- **The default network.** A service with no `networks:` key joins compose's default network,
  so the module enables `networks.default`. pin-oci attaches *every* service to it, so a
  service that compose keeps off it is reported.

Exit codes: 0 converted, 1 `--strict` with warnings, 2 error. The command needs neither `nix`
nor `skopeo`, and Nix evaluation never reads a compose file.

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
| `no nixosConfigurations host of '…' uses pin-oci` | The flake has no host with `virtualisation.pin-oci.enable = true`; or target one with `--flake .#<host>`. |
| `'github:…' is not a local flake: pass --file <path>` | Remote flake references need an explicit `--file`. |
| `cannot run skopeo` / `cannot run nix` | The binary is not in `PATH` (not needed with the packaged `pin-oci`, which wraps `skopeo`). |
| `skopeo failed … manifest unknown` | The tag does not exist in the registry. |
| `no manifest for the host platform …` | The image is not published for your architecture. |
| `[pin-oci] dependsOn names a service that is not in the project` | Typo in a project `dependsOn`; names are the service names without the project prefix. |
| `[pin-oci] External resources missing …` | An `external = true` resource is not declared in `virtualisation.pin-oci.networks` / `volumes`. |

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
$ nix run . -- lock --check --flake path/to/flake#host
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
| Nix modules | `tests/nix/default.nix`, `infra.nix`, `projects.nix`, `resources.nix` (run by `nix flake check`) | Reference normalization, module evaluation against fixture lockfiles (present / missing / wrong architecture / corrupt), the generated systemd units, laziness of `lockTargets`, container ordering, and the exact expression the CLI evaluates (`src/apply.nix`, shared with Rust). |
| Overlay | `tests/nix/overlay.nix` | `overlays.default` provides `pkgs.pin-oci` (same derivation as the package), adds nothing else, and `environment.systemPackages = [ pkgs.pin-oci ]` resolves; without the overlay `pkgs` has no `pin-oci`. |
| Convert | `src/convert.rs`, `src/render.rs`, `tests/convert_cli.rs`, `tests/nix/convert.nix`, `tests/fixtures/compose/` | Every mapped key and every warning, the errors (interpolation, rewritten numbers, no image, bad names), YAML 1.2 booleans and merge keys, Nix escaping of hostile values; the binary against reference files (compose in, Nix out, byte for byte); and those same reference modules evaluated by the real module, offline. |
| Completions | `tests/completions_cli.rs`, `tests/nix/completions.nix` | `pin-oci completions <shell>` for every shell (no `nix`, no `skopeo`, usage errors), and the package ships the bash, zsh and fish scripts, equal to what the packaged binary prints. |
| Binary cache | `tests/nix/binary-cache.nix` | The flake's `nixConfig`: substituter, public key, key matching its substituter, `extra-*` only (defaults never replaced). |
| Generated scripts | `tests/nix/infra-scripts.nix` | The network and volume unit scripts executed against a stateful fake backend: idempotence, flags, recreation, failure propagation. |
| NixOS VM | `tests/nix/vm-infra.nix`, `vm-projects.nix` | Real `podman` and `docker` in a VM: resources created at boot, flags applied, units idempotent, container units ordered after their resources; for projects, prefixed and external resources, `dependsOn` ordering, and services resolving each other by bare name on the default network. Needs KVM (`system-features = kvm nixos-test`). |

Run a single check, for instance:

```console
$ nix build .#checks.x86_64-linux.module-tests -L   # fast, evaluation only
$ nix build .#checks.x86_64-linux.infra-scripts -L
$ nix build .#checks.x86_64-linux.vm-infra -L        # boots two VMs, about a minute
$ nix build .#checks.x86_64-linux.vm-projects -L
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
│   ├── check.rs        # `lock --check`: declared images vs lockfile
│   ├── list.rs         # `lock --list`: declared and locked images with their status
│   ├── convert.rs      # `convert`: compose subset to a project, warnings and errors
│   ├── render.rs       # `convert`: project to Nix text, string escaping
│   ├── lock.rs         # `lock`: resolve missing, prune, write
│   ├── nix.rs          # nix eval boundary, <flake>#<host> handling
│   ├── registry.rs     # skopeo boundary, index / single-manifest resolution
│   ├── lockfile.rs     # lockfile model, platform mapping, deterministic write
│   └── apply.nix       # expression the CLI evaluates on the configuration
├── modules/
│   ├── default.nix     # module entry point
│   ├── containers.nix  # milestone 1: options, assertion, lowering
│   ├── infra.nix       # milestone 2: network / volume units, container ordering
│   ├── projects.nix    # milestone 3: projects compiled to containers, networks, volumes
│   ├── resources.nix   # pure helpers: name rule, network / volume reference parsing, rewriting
│   ├── container-module.nix  # oci-containers options shared by containers and services
│   └── refs.nix        # pure image reference normalization
├── tests/              # Rust e2e + fixtures; nix/ holds the module, script and VM tests
├── flake.nix           # nixosModules.default, overlays.default, packages.pin-oci, devShell, checks
├── devenv.nix          # development environment
└── BIBLE.md            # specification
```
