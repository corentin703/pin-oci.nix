# Technical Specification & Architecture Bible — `pin-oci.nix`

NixOS module and Rust CLI tooling for deterministic pinning of container images and declarative composition on top of `virtualisation.oci-containers`.

---

## 1. Guiding Principles

1. **No reinvention of execution:** `virtualisation.oci-containers` (Docker/Podman) remains the sole downstream execution engine. The `virtualisation.oci-containers.backend` option remains the single source of truth for the runtime.
2. **Strict purity at Nix evaluation time:** No network requests during `nix build` or `nixos-rebuild`. Resolving remote tags to immutable digests (`sha256:...`) is delegated to a dedicated Rust CLI running outside the sandbox. The same goes for the hash of an image fetched into the store (`pullImage`).
3. **Committed lockfile (`pin-oci.lock`):** A standard JSON file documenting multi-architecture resolutions, natively readable via `builtins.fromJSON` with no plugin and no IFD (*Import From Derivation*).
4. **Strict fail-fast:** Any image declared without a valid digest for the host platform triggers an immediate blocking assertion at evaluation time, providing the exact command to run.
5. **Composability through lowering:** Higher-level abstractions (projects, volumes, networks) compile down to standard NixOS primitives (systemd units and flat containers).

---

## 2. Lockfile Format (`pin-oci.lock`)

Location: root of the flake, next to `flake.lock` (`<flake root>/pin-oci.lock`). The module's `lockFile` option has no default and must point to it (see §4.1).

### Canonical key

The key of an entry is the **canonical reference** `<registry>/<repository>:<tag>`. It is generated **only on the Nix side** (`modules/containers.nix`); the Rust CLI never parses or normalizes image references, it consumes the `lockTargets` evaluated from Nix (see §4.3).

Normalization rules:

* no registry in the reference → `docker.io`;
* `docker.io` with a single-segment repository → `library/` prefix;
* no tag → `latest`;
* a reference already carrying `@sha256:...` is considered pinned: it is passed through unchanged and has no entry in the lockfile.

```json
{
  "version": 1,
  "images": {
    "docker.io/library/caddy:2-alpine": {
      "registry": "docker.io",
      "repository": "library/caddy",
      "tag": "2-alpine",
      "arch": {
        "x86_64-linux": {
          "digest": "sha256:d8a26e8499...",
          "os": "linux",
          "architecture": "amd64",
          "nixHash": "sha256-4Y35qO8R..."
        },
        "aarch64-linux": {
          "digest": "sha256:f1c34a9912...",
          "os": "linux",
          "architecture": "arm64"
        }
      }
    }
  }
}

```

### `nixHash` (images pulled into the store)

`nixHash` is optional and per platform. It is the SRI `sha256` that `dockerTools.pullImage` checks: the hash of the archive `skopeo copy` produces for that platform, which cannot be derived from the registry digest and requires fetching the image. It is present only for images some container pulls into the store (`pullImage`, see §4.1); the CLI computes it, never the module. It is tied to its `digest`: a digest that moves invalidates it.

### Architecture mapping table

| OCI `platform.os` / `architecture` | Nix `pkgs.stdenv.hostPlatform.system` |
| --- | --- |
| `linux` / `amd64` | `x86_64-linux` |
| `linux` / `arm64` | `aarch64-linux` |
| `linux` / `arm` (v7) | `armv7l-linux` |
| `linux` / `riscv64` | `riscv64-linux` |

Index entries whose platform is `unknown/unknown` (attestation manifests) are ignored.

---

## 3. Technical Breakdown by Milestone

```
┌─────────────────────────────────────────────────────────────┐
│ Milestone 3: virtualisation.pin-oci.projects.<name>         │
│ - Compose abstraction (linked services, project namespace)  │
│ - Automatic local network & systemd ordering (dependsOn)    │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Milestone 2: Shared infrastructure                          │
│ - virtualisation.pin-oci.networks.<name>                    │
│ - virtualisation.pin-oci.volumes.<name>                     │
│ - Idempotent systemd init units (exists || create)          │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌─────────────────────────────────────────────────────────────┐
│ Milestone 1: Pinning engine (Core)                          │
│ - virtualisation.pin-oci.containers.<name>                  │
│ - Override of the image option (tag -> @sha256)             │
│ - Pure evaluation assertion & Rust CLI (pin-oci lock)       │
│ - Direct lowering to virtualisation.oci-containers          │
└─────────────────────────────────────────────────────────────┘

```

---

## 4. Milestone 1 — Version Pinning (Core)

### 4.1 NixOS Interface

The submodule replicates the exact options of `virtualisation.oci-containers.containers.<name>`. They are derived from the host configuration (`options.virtualisation.oci-containers.containers`), never imported through `<nixpkgs>` / `NIX_PATH`. The `image` field receives an unresolved string (`"image:tag"`).

Consumption as a flake input:

```nix
inputs = {
  nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";

  pin-oci.url = "github:corentin703/pin-oci.nix";
};
```

There is deliberately no `inputs.nixpkgs.follows`. The module takes its options and `pkgs` from the host configuration, never from the flake's own nixpkgs, so `follows` aligns nothing there. It would only make Nix build the CLI against the host's nixpkgs, which differs from what CI built and defeats the binary cache.

The CLI is also exposed as an overlay, **opt-in**: `overlays.default` adds a single attribute, `pin-oci`, which is `packages.<system>.pin-oci` unchanged (same derivation, so the binary cache applies). Neither the module nor the flake applies it implicitly; the host does:

```nix
nixpkgs.overlays = [ inputs.pin-oci.overlays.default ];
environment.systemPackages = [ pkgs.pin-oci ];
```

`lockFile` has **no default** (a module cannot know the root of the user's flake) and is mandatory when `enable = true`. Use `./pin-oci.lock` when the configuration file sits at the flake root, otherwise `self + "/pin-oci.lock"` (`self` must be passed through `specialArgs`). In a flake, `builtins.pathExists` does not see files untracked by git: after `pin-oci lock`, run `git add pin-oci.lock`.

```nix
virtualisation.pin-oci = {
  enable = true;
  lockFile = ./pin-oci.lock; # mandatory, no default

  containers.traefik = {
    image = "traefik:v3.0";
    ports = [ "80:80" "443:443" ];
    volumes = [ "/var/run/docker.sock:/var/run/docker.sock:ro" ];
  };
};

```

### 4.2 Nix Modules (`modules/containers.nix`, `modules/refs.nix`)

**`modules/refs.nix`** holds the pure reference normalization: the only place where image references are parsed, unit-tested on its own. It applies the rules of §2 and flags a reference that already carries a digest as `pinned`, to be passed through untouched.

**`modules/containers.nix`** is the engine.

* **Options.** `virtualisation.pin-oci.containers` takes exactly the options of `oci-containers`, derived from the host's own module (`getSubModules`) so that none is lost or added by hand. `lockFile` is required (no default). `system` and `lockTargets` are read-only and are the CLI's whole view of the configuration.
* **Targets.** `lockTargets` maps each canonical key of an unpinned image to `{ registry, repository, tag, pullImage }`. It never depends on the lockfile.
* **Lowering.** Each container is lowered to `virtualisation.oci-containers.containers.<name>` (the only runtime layer, never reimplemented), with `image` replaced by `<registry>/<repository>@<digest of the host platform>`; a pinned reference is left as is.
* **Fail-fast.** An image with no entry for the host platform fails evaluation with an assertion that lists the images and gives the exact command (`pin-oci lock`, then `git add pin-oci.lock`). Nothing is lowered while an assertion fails.

**Laziness guarantee.** Assertions are only forced by `system.build.toplevel`. `lockTargets` does not depend on `lockData`, so `nix eval ...lockTargets` succeeds with a missing lockfile, while `nixos-rebuild` fails with the message above.

**`pullImage`** (optional; `virtualisation.pin-oci.pullImage`, `false` by default, overridable per container). Instead of letting the runtime pull a digest, the module generates a `dockerTools.pullImage` from the lock entry (digest and `nixHash`), sets `imageFile` to it and runs the image under its `<registry>/<repository>:<tag>` name, which `oci-containers` finds once the archive is loaded. The option is consumed by the module, never forwarded. In `lockTargets`, `pullImage` is true when any container sharing the image pulls it; the CLI reads it to know which hashes to compute. A pulled image with no `nixHash` for the host platform fails evaluation with the command to run, as does `pullImage` on a reference that already carries a digest. Evaluation stays pure: the hash comes from the lockfile and the fetch happens in the build.

### 4.3 Rust CLI Specification (`pin-oci`)

The CLI never parses image references and holds no registry client or credentials: it asks Nix for the canonical targets and delegates registry access and authentication to `skopeo`, using the host's own credentials and credential helpers.

#### Commands:

* `pin-oci lock [--flake <ref>] [--file <path>]`, and its subcommands `lock update [<image>...]`, `lock check` and `lock list` below. `--flake` and `--file` are accepted before or after the subcommand, as in `nix flake`:
1. Evaluates the configuration without building, retrieving the host system and the targets in a single evaluation (per host; see the end of this section for `--flake` without `<host>`):
```bash
nix eval --json <flake>#nixosConfigurations."<host>".config \
  --apply 'c: { system = c.virtualisation.pin-oci.system; targets = c.virtualisation.pin-oci.lockTargets; }'

```


2. Resolves only the targets missing from the lockfile (absent key, or no entry for the host system); existing entries are never re-resolved, unless `lock update` selects them (see below).
3. For each, runs `skopeo inspect --raw docker://<registry>/<repository>:<tag>`:
   * **index** (`manifests[]`): takes each entry's `digest` and `platform.{os,architecture,variant}`, maps platforms through the table of §2 and ignores unmappable entries (`unknown/unknown` attestations, `s390x`, `arm` v6...);
   * **single manifest** (no `manifests`): runs `skopeo inspect --no-tags` and reads `Digest`, `Os`, `Architecture`, `Variant`; an unmappable platform is an error.
4. Fails if an image has no manifest for the host system, or if two manifests map to the same system.
5. For every target whose `lockTargets` entry has `pullImage = true` (any declaring container pulls it, whatever the host), computes the `nixHash` of each platform of the declaring hosts that has none, by replaying the fetch `pullImage` performs for that platform and hashing the resulting archive. A digest that did not move keeps its hash, so a plain `lock` fetches only what has none; `lock update` recomputes the hash of every platform whose digest moved. Any failure fails the command and leaves the lockfile untouched.
6. Prunes lockfile entries absent from `lockTargets`.
7. Writes deterministic JSON (`BTreeMap` order, fixed field order, 2-space indent, final newline), atomically (temporary file then rename), and leaves the file untouched when the content is unchanged.

**`lock update [<image>...]`** is the only way a locked digest changes. It re-resolves locked images, moving each tag to its current digests, and replaces the whole entry (every platform of the index, not only the host's). With no argument it covers every declared image; otherwise only the given images, each named by its **canonical key**, exactly as it appears in the lockfile and in `lockTargets`. The CLI still does not normalize: a key that no declared image has (`caddy:2-alpine` instead of `docker.io/library/caddy:2-alpine`) is an error that lists the declared keys, raised before any registry call, with the lockfile untouched. Missing images are still resolved and undeclared entries still pruned (steps 2 and 6). The output lists, for each refreshed image whose digests moved, the platforms concerned (`~ <key>`, then `<system> <old> -> <new>`), and counts the images already current. The image arguments belong to `update` alone. **`lock update --dry`** runs the same resolution and prints the same report (headed `dry run, nothing written; would write`, or `would stay up to date`), but never writes the lockfile and never fetches an image: step 5 is skipped, so `nixHash` values are neither computed nor reported. Errors are those of the real run; the exit code is 0 whenever it succeeds. `--dry` belongs to `update` alone.

**All or nothing:** if any image fails to resolve, every failure is reported, nothing is written and the exit code is 2.

`--file` defaults to `<flake root>/pin-oci.lock` for a local flake reference; it is mandatory for a remote reference.

**Registry access and authentication** are delegated to `skopeo` (runtime dependency, wrapped into the `pin-oci` package and provided by the dev shells), which reads the host's own credentials (`REGISTRY_AUTH_FILE`, `${XDG_RUNTIME_DIR}/containers/auth.json`, `~/.docker/config.json`) and credential helpers. The CLI embeds no HTTP client and no credential handling, and does not depend on `virtualisation.oci-containers.backend`.


* `pin-oci lock check [--flake <ref>] [--file <path>]` (there is no top-level `check` subcommand):
Checks that every key of `lockTargets` has an entry for the platform of every host declaring it in the lockfile (and a `nixHash` for it when the target has `pullImage = true`), without modifying any file. It evaluates the same `{ system; targets }` as `lock` (only the target keys are used), never the lockfile. Lockfile entries absent from `lockTargets` are ignored (only `lock` prunes them); a missing lockfile counts as empty.
Exit codes: 0 = in sync, 1 = out of sync, 2 = error (nix failure, unreadable or invalid lockfile, remote flake without `--file`). Suited for CI steps.

* `pin-oci lock list [--flake <ref>] [--file <path>]`:
Lists every key that is declared (`lockTargets`) or locked, sorted, as a table `STATUS IMAGE HOSTS DIGEST`. `HOSTS` is the comma-separated list of hosts declaring the image (`-` for a stale one). Status is relative to the platforms of those hosts: `locked` (declared, has a digest for each of them), `missing` (declared, no entry for at least one, even if other systems are locked), `no-hash` (locked, but pulled into the store and lacking the `nixHash` of one of those platforms) or `stale` (locked, no longer declared). `DIGEST` shows, in full, one `<system> <digest>` per line for each of those platforms the lockfile has (every locked platform for a stale image). It evaluates the same `{ system; targets }` as `check`, reads the lockfile (absent = empty) and writes nothing; it takes no `Runtime`, so it cannot contact a registry. Exit codes: 0 on success whatever the statuses (use `lock check` to gate CI), 2 on error. 

* `pin-oci completions <shell>`:
Prints the completion script for `bash`, `zsh`, `fish`, `elvish` or `powershell` on stdout (`clap_complete`, generated from the `clap` definition so it cannot drift from the CLI). It touches no file and needs neither `nix` nor `skopeo`; an unknown or missing shell is a usage error (exit 2). The Nix package installs the bash, zsh and fish scripts (`installShellCompletion`, produced by the built binary itself), so they are in place as soon as `pin-oci` is in `environment.systemPackages`.

`--flake` defaults to `.`. With `<flake>#<host>` it targets `nixosConfigurations."<host>".config` alone. Without `#<host>` (including the default), the CLI aggregates every host of the flake:
1. lists them with `nix eval --json <flake>#nixosConfigurations --apply builtins.attrNames`;
2. evaluates each one with the same contract, which yields `null` for a host that does not use pin-oci (module absent or `enable = false`). Such hosts are ignored and named in the output (`ignored, pin-oci not enabled: <hosts>`), never an error;
3. merges the hosts into the **union** of their targets, written to a single lockfile. The lockfile records platforms, never hosts. An image is missing when it has no entry for the platform of any host declaring it, `lock` fails if its index lacks the platform of one of those hosts, and pruning only drops keys declared by no host.

All or nothing: if any host fails to evaluate, every failure is reported, nothing is resolved or written and the exit code is 2. It is also an error (exit 2) when the flake has no `nixosConfigurations` or no host enables pin-oci.

---

## 5. Milestone 2 — Global Resources (`networks` & `volumes`)

Introduces shared infrastructure primitives, leveraging the native `virtualisation.oci-containers.backend` option (`docker` or `podman`).

### 5.1 Declarative Interface

```nix
virtualisation.pin-oci = {
  enable = true;

  networks.app-net = {
    subnet = "10.89.1.0/24"; # optional
    internal = false;         # optional, cuts the Internet gateway if true
  };

  volumes.postgres_data = {
    driver = "local";         # optional
  };

  containers.db = {
    image = "postgres:16-alpine";
    volumes = [ "postgres_data:/var/lib/postgresql/data" ];
    extraOptions = [ "--network=app-net" ];
  };
};

```

### 5.2 Systemd Lowering

Nothing is generated unless `virtualisation.pin-oci.enable = true`. For each defined resource, a `oneshot` systemd unit with `RemainAfterExit = true` and `wantedBy = [ "multi-user.target" ]` is generated (so declared resources exist even when no container references them). The unit body is idempotent: it creates the resource only when it is absent.

* **Network:** `${backend}-network-${name}.service`
```bash
${backend} network inspect ${name} >/dev/null 2>&1 || ${backend} network create [--subnet <subnet>] [--internal] ${name}

```


* **Volume:** `${backend}-volume-${name}.service`
```bash
${backend} volume inspect ${name} >/dev/null 2>&1 || ${backend} volume create --driver <driver> ${name}

```


`${backend}` is the store path of `pkgs.${backend}` (`.../bin/${backend}`). `inspect` is used instead of `exists` because Docker has no `exists` subcommand (verified with Docker 29.8): `inspect` exits non-zero when the resource is absent on both backends. With the `docker` backend the units are ordered `after` and `requires` `docker.service`.

Names must match `[a-zA-Z0-9][a-zA-Z0-9_.-]*` (the Docker/Podman rule, also safe in unit names); any other name fails an assertion at evaluation.

* **Container ordering:**
A `virtualisation.pin-oci.containers.<c>` receives, on its `serviceName` unit (`${backend}-${c}.service`), `after` and `requires` on the unit of every declared resource it references:
```nix
after = [ "${backend}-network-app-net.service" "${backend}-volume-postgres_data.service" ];
requires = [ "${backend}-network-app-net.service" "${backend}-volume-postgres_data.service" ];

```


A container references:
  * a network through the `networks` option, `extraOptions = [ "--network=<n>" ]`, `"--net=<n>"`, or the two-element form `[ "--network" "<n>" ]` (anything after a `:` in the value, such as `:alias=db`, is ignored);
  * a volume through the source of `volumes = [ "<name>:<path>[:opts]" ]`; bind mounts (`/…`, `./…`, `~/…`) never match since names cannot start with those characters.

Only names declared in `virtualisation.pin-oci.networks` / `volumes` produce a dependency; any other name (`host`, a pre-existing volume, a bind mount) is left alone. Nothing is added to a container that was not lowered (unlocked image).

**Limitation:** resources are created by root. A rootless Podman container (`podman.user != "root"`) does not see them.

---

## 6. Milestone 3 — Compose Stacks (`projects`)

Syntactic sugar structuring sets of interdependent containers within a shared namespace.

### 6.1 Declarative Interface

```nix
virtualisation.pin-oci.projects.monitoring = {
  enable = true;
  networks.default.enable = true; # Creates 'monitoring-default', attached to every service

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
  # volumes.shared_data.external = true;  # global resource, no prefix
};

```

### 6.2 Compilation Rules to Milestones 1 & 2

A project compiles into plain `virtualisation.pin-oci.containers`, `networks` and `volumes` entries (nothing is generated unless both `virtualisation.pin-oci.enable` and `projects.<p>.enable` are set). Milestones 1 and 2 then do the rest: locking, fail-fast, units, ordering.

Project, service, network and volume names must match `[a-zA-Z0-9][a-zA-Z0-9_.-]*`. Names generated by different projects must be unique (e.g. project `a-b` with service `c` and project `a` with service `b-c` both give `a-b-c`); a collision is an assertion failure. A collision with a hand-written `containers` / `networks` / `volumes` entry is reported by the module system as conflicting definitions.

1. **Name prefixing and `external`:**
* Container: `<project>-<service>` (e.g. `monitoring-grafana`). A service accepts exactly the options of a container (§4.1).
* Project `volumes.<v>` and `networks.<n>` carry an `external` flag (default `false`); volumes also take `driver`, networks `subnet` and `internal`.
* `external = false`: the resource is prefixed with the project (volume `<project>_<volume>`, e.g. `monitoring_grafana_data`; network `<project>-<network>`) and declared in the global `virtualisation.pin-oci.{networks,volumes}`, which generates its init unit (§5).
* `external = true`: no prefix, nothing generated; an assertion requires the resource to exist in the global `virtualisation.pin-oci.{networks,volumes}`.
* References inside the services are rewritten to the prefixed name when they designate a non-external project resource: the `volumes` sources (`grafana_data:/var/lib/grafana`), the `networks` entries and the network forms of `extraOptions` (`--network=`, `--net=`, two-element form), keeping any suffix (`:alias=db`). Anything else (external resources, bind mounts, undeclared names) is passed through unchanged.


2. **Local DNS resolution:**
`networks.default.enable = true` creates the `<project>-default` network (never external, `enable` is `false` by default for `default` only) and attaches every service to it. Containers are named `<project>-<service>`, and Docker/Podman only resolve a container name or a network alias: each service therefore joins the default network with the alias `<service>`, so `prometheus` resolves inside the project. The alias syntax depends on the backend: Podman `--network=<net>:alias=<service>`, Docker `--network=name=<net>,alias=<service>`. Other project networks give no automatic alias (write one in the `networks` entry if needed).
3. **`dependsOn` translation:**
`dependsOn = [ "prometheus" ]` is rewritten to the flat name `<project>-prometheus` and handed to the native `oci-containers` option, which adds the target unit to both `After` and `Requires`:
```nix
after = [ "${backend}-${project}-${depService}.service" ];
requires = [ "${backend}-${project}-${depService}.service" ];

```
A `dependsOn` naming a service absent from the project is an assertion failure.


4. **Project target:**
Each enabled project generates `<backend>-<project>.target` (e.g. `podman-monitoring.target`), `wantedBy` `multi-user.target` and `wants` every service unit of the project. Each service unit is `partOf` the target, so `start`, `stop` and `restart` on the target apply to all services. The target runs nothing itself. The service unit is the container's `serviceName` (an explicit one is kept). Network and volume init units are not part of the target: services already `require` them. No target is generated for a disabled project.

5. **Feeding the CLI:**
Project services are lowered into `virtualisation.pin-oci.containers`, so `virtualisation.pin-oci.lockTargets` covers them with no extra code; `pin-oci lock` consumes that single option.

### 6.3 Conversion from compose (`pin-oci convert`)

`pin-oci convert <file> [--name <project>] [--output <path>] [--strict]` turns a docker-compose file into a module declaring a `virtualisation.pin-oci.projects.<name>`. It is a convenience of the CLI: Nix evaluation never reads a compose file, so evaluation purity (§1) is untouched. The flow is `convert`, review, then `pin-oci lock` (images stay floating tags until locked). It is a **useful subset of compose**, not compose.

* **Never silent.** Every key that is not converted is reported on stderr as `pin-oci: warning: <path>: not supported, ignored`, at every level (`services.web.healthcheck`, `services.web.networks.back.aliases`, top-level `secrets`...). `x-*` extensions and the obsolete `version` are not noise. `--strict` turns any warning into exit 1 and writes nothing.
* **Faithful or refused.** A value that cannot be written into a static module as it is makes the conversion fail (exit 2), listing every offending path: variable interpolation (`${VAR}`, `$VAR`; `$$` is a literal `$`), and unquoted numbers YAML would not hand back unchanged (`1.10` is read as `1.1`, `007` as `7`). A service without `image` fails too (`build` is named: pin-oci only runs images).
* **YAML semantics are compose's.** Only `true` and `false` are booleans (`no`, `yes`, `on`, `n` stay text, so `DEBUG: no` is the string `no`), anchors and merge keys (`<<: *x`) are resolved.
* **Mapping.** `image`; `command` and `entrypoint` (a list, or a string split like a shell: `entrypoint` becomes the first word, the others lead `cmd`, as `oci-containers` takes a single entrypoint word); `environment` and `labels` (map or `KEY=value` list; an entry with no value is reported); `ports` and `volumes` (short and long syntax; `tmpfs` and relative bind mounts are reported); `depends_on` (list or map, only the start order is kept: any `condition` but `service_started` is reported); `networks`; `user`; `working_dir` (`workdir`). Top-level `volumes` (`external`, `driver`) and `networks` (`external`, `internal`, `bridge` driver, first `ipam` subnet), and `name`.
* **The default network.** `networks.default.enable = true` is emitted when any service joins compose's default network (no `networks:` key, or `default` listed). pin-oci then attaches every service to it, so a service that compose keeps off it is reported.
* **Project name:** `--name`, else the file's `name:`, else the directory of the file; it must obey the name rule of §5.2, as every service, volume and network name.
* **Output.** Stdout, or `--output <path>`. Deterministic: sorted names, fixed field order, the file name (not its path) in the header. Every string is a Nix string literal with `\`, `"` and `${` escaped: no compose value can run Nix code.
* **Exit codes:** 0 converted, 1 `--strict` and something was not converted, 2 error (unreadable or invalid YAML, an unfaithful value, a missing image, a bad name).

---

## 7. Git Repository Layout

```text
pin-oci.nix/
├── Cargo.toml, Cargo.lock
├── src/                # the Rust CLI
│   ├── main.rs         # entry point, clap subcommands (lock, lock update/list/check, convert, completions)
│   ├── lock.rs         # `lock` and `lock update`: resolve, refresh, hash images to pull, prune, write
│   ├── check.rs        # `lock check`: targets vs lockfile
│   ├── list.rs         # `lock list`: declared and locked images with their status
│   ├── lockfile.rs     # lockfile model, platform mapping, deterministic serialization
│   ├── nix.rs          # boundary around `nix eval`: system and lockTargets, multi-host union
│   ├── registry.rs     # boundary around skopeo: manifest resolution, archive hash
│   ├── convert.rs      # `convert`: compose subset to a project (pure)
│   ├── render.rs       # `convert`: project to Nix text (pure)
│   └── apply*.nix      # the expressions the CLI evaluates on a configuration
├── modules/            # the NixOS modules
│   ├── default.nix     # entry point aggregating the modules
│   ├── refs.nix        # pure image reference normalization
│   ├── containers.nix  # milestone 1: engine, assertions, lowering to oci-containers
│   ├── container-module.nix  # oci-containers options shared by containers and services, plus pullImage
│   ├── resources.nix   # pure helpers: names, network / volume references
│   ├── infra.nix       # milestone 2: systemd networks and volumes
│   └── projects.nix    # milestone 3: compose-style projects lowered to the above
├── tests/              # Rust end to end (fake nix / skopeo on PATH), fixtures; nix/ holds the module, script and VM tests
├── flake.nix           # nixosModules.default, overlays.default, packages.pin-oci, devShell, checks
└── README.md
```
