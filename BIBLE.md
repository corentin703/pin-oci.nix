# Technical Specification & Architecture Bible — `pin-oci.nix`

NixOS module and Rust CLI tooling for deterministic pinning of container images and declarative composition on top of `virtualisation.oci-containers`.

---

## 1. Guiding Principles

1. **No reinvention of execution:** `virtualisation.oci-containers` (Docker/Podman) remains the sole downstream execution engine. The `virtualisation.oci-containers.backend` option remains the single source of truth for the runtime.
2. **Strict purity at Nix evaluation time:** No network requests during `nix build` or `nixos-rebuild`. Resolving remote tags to immutable digests (`sha256:...`) is delegated to a dedicated Rust CLI running outside the sandbox.
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
          "architecture": "amd64"
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

`modules/refs.nix` holds the pure reference normalization (the only place references are parsed, unit-tested on its own):

```nix
{ lib }:

{
  # Canonical form of an image reference (BIBLE §2). The only place where references are parsed.
  #   redis:7                   -> docker.io/library/redis:7
  #   localhost:5000/foo        -> localhost:5000/foo:latest
  #   redis@sha256:...          -> pinned = true (passed through untouched by the module)
  parseRef =
    ref:
    let
      pinned = lib.hasInfix "@" ref;
      withoutDigest = builtins.head (lib.splitString "@" ref);
      parts = lib.splitString "/" withoutDigest;
      first = builtins.head parts;
      # Docker's rule: the first segment is a registry if it looks like a host.
      hasRegistry =
        builtins.length parts > 1
        && (lib.hasInfix "." first || lib.hasInfix ":" first || first == "localhost");
      registry = if hasRegistry then first else "docker.io";
      path = if hasRegistry then lib.concatStringsSep "/" (builtins.tail parts) else withoutDigest;
      lastSegment = lib.last (lib.splitString "/" path);
      hasTag = lib.hasInfix ":" lastSegment;
      tag = if hasTag then lib.last (lib.splitString ":" lastSegment) else "latest";
      untagged = if hasTag then lib.removeSuffix ":${tag}" path else path;
      repository =
        if registry == "docker.io" && !(lib.hasInfix "/" untagged) then "library/${untagged}" else untagged;
    in
    if tag == "" || untagged == "" || lib.hasSuffix "/" untagged then
      throw "[pin-oci] invalid image reference '${ref}'"
    else
      {
        inherit
          pinned
          registry
          repository
          tag
          ;
        key = "${registry}/${repository}:${tag}";
      };
}
```

`modules/containers.nix` is the engine. Options come from the host's `oci-containers` module through `getSubModules` (verified against nixpkgs: the container options are a function module `{ name, ... }:`). `virtualisation.pin-oci.system` and `lockTargets` are the CLI's whole view of the configuration; neither depends on the lockfile.

```nix
{
  config,
  options,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.virtualisation.pin-oci;
  inherit (import ./refs.nix { inherit lib; }) parseRef;

  currentSystem = pkgs.stdenv.hostPlatform.system;

  # Read lazily: only the assertion and the lowering depend on the lockfile.
  lockData =
    if builtins.pathExists cfg.lockFile then
      builtins.fromJSON (builtins.readFile cfg.lockFile)
    else
      {
        version = 1;
        images = { };
      };

  # Same options as oci-containers, derived from the host configuration.
  pinContainerModule = {
    imports = options.virtualisation.oci-containers.containers.type.nestedTypes.elemType.getSubModules;
  };

  unpinned = lib.filterAttrs (_: c: !(parseRef c.image).pinned) cfg.containers;

  # Consumed by the pin-oci CLI. MUST NOT depend on lockData.
  lockTargets = lib.mapAttrs' (
    _: c:
    let
      r = parseRef c.image;
    in
    lib.nameValuePair r.key { inherit (r) registry repository tag; }
  ) unpinned;

  isMissing =
    key:
    let
      entry = (lockData.images or { }).${key} or null;
    in
    entry == null || !((entry.arch or { }) ? ${currentSystem});

  missingKeys = lib.filter isMissing (builtins.attrNames lockTargets);
  formattedList = lib.concatMapStringsSep "\n  - " (k: k) missingKeys;

  # Replace the floating tag by the locked digest.
  resolveContainer =
    _: containerCfg:
    let
      r = parseRef containerCfg.image;
      base = removeAttrs containerCfg [ "_module" ];
    in
    if r.pinned then
      base
    else
      base
      // {
        image = "${r.registry}/${r.repository}@${lockData.images.${r.key}.arch.${currentSystem}.digest}";
      };
in
{
  options.virtualisation.pin-oci = {
    enable = lib.mkEnableOption "pin-oci deterministic container manager";

    lockFile = lib.mkOption {
      type = lib.types.path;
      example = lib.literalExpression "./pin-oci.lock";
      description = ''
        Path to pin-oci.lock, at the flake root next to flake.lock.
        No default: set it to `./pin-oci.lock`, or `self + "/pin-oci.lock"`.
      '';
    };

    containers = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule pinContainerModule);
      default = { };
      description = "Container specifications (identical to oci-containers).";
    };

    system = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      description = "Platform the lockfile is checked against, consumed by the pin-oci CLI.";
    };

    lockTargets = lib.mkOption {
      type = lib.types.attrsOf lib.types.attrs;
      readOnly = true;
      description = "Canonical image keys to resolve, consumed by the pin-oci CLI.";
    };
  };

  config = lib.mkMerge [
    {
      virtualisation.pin-oci = {
        system = currentSystem;
        inherit lockTargets;
      };
    }

    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = missingKeys == [ ];
          message = ''

            [pin-oci] Missing or unpinned images for '${currentSystem}' in ${toString cfg.lockFile}:
              - ${formattedList}

            Synchronize the lock file with the command:
              pin-oci lock
            then make it visible to the flake:
              git add pin-oci.lock
          '';
        }
      ];

      virtualisation.oci-containers.containers =
        if missingKeys == [ ] then lib.mapAttrs resolveContainer cfg.containers else { };
    })
  ];
}
```

**Laziness guarantee.** Assertions are only forced by `system.build.toplevel`. `lockTargets` does not depend on `lockData`, so `nix eval ...lockTargets` succeeds with a missing lockfile, while `nixos-rebuild` fails with the message above.

### 4.3 Rust CLI Specification (`pin-oci`)

The CLI never parses image references and holds no registry client or credentials: it asks Nix for the canonical targets and delegates registry access and authentication to `skopeo`, using the host's own credentials and credential helpers.

#### Commands:

* `pin-oci lock [--flake <ref>] [--file <path>] [--update [<image>...]]`, and its mode `--check` below:
1. Evaluates the configuration without building, retrieving the host system and the targets in a single evaluation (per host; see the end of this section for `--flake` without `<host>`):
```bash
nix eval --json <flake>#nixosConfigurations."<host>".config \
  --apply 'c: { system = c.virtualisation.pin-oci.system; targets = c.virtualisation.pin-oci.lockTargets; }'

```


2. Resolves only the targets missing from the lockfile (absent key, or no entry for the host system); existing entries are never re-resolved, unless `--update` selects them (see below).
3. For each, runs `skopeo inspect --raw docker://<registry>/<repository>:<tag>`:
   * **index** (`manifests[]`): takes each entry's `digest` and `platform.{os,architecture,variant}`, maps platforms through the table of §2 and ignores unmappable entries (`unknown/unknown` attestations, `s390x`, `arm` v6...);
   * **single manifest** (no `manifests`): runs `skopeo inspect --no-tags` and reads `Digest`, `Os`, `Architecture`, `Variant`; an unmappable platform is an error.
4. Fails if an image has no manifest for the host system, or if two manifests map to the same system.
5. Prunes lockfile entries absent from `lockTargets`.
6. Writes deterministic JSON (`BTreeMap` order, fixed field order, 2-space indent, final newline), atomically (temporary file then rename), and leaves the file untouched when the content is unchanged.

**`--update [<image>...]`** is the only way a locked digest changes. It re-resolves locked images, moving each tag to its current digests, and replaces the whole entry (every platform of the index, not only the host's). With no argument it covers every declared image; otherwise only the given images, each named by its **canonical key**, exactly as it appears in the lockfile and in `lockTargets`. The CLI still does not normalize: a key that no declared image has (`caddy:2-alpine` instead of `docker.io/library/caddy:2-alpine`) is an error that lists the declared keys, raised before any registry call, with the lockfile untouched. Missing images are still resolved and undeclared entries still pruned (steps 2 and 5). The output lists, for each refreshed image whose digests moved, the platforms concerned (`~ <key>`, then `<system> <old> -> <new>`), and counts the images already current. `--update` excludes `--check`; the image arguments require `--update`.

**All or nothing:** if any image fails to resolve, every failure is reported, nothing is written and the exit code is 2.

`--file` defaults to `<flake root>/pin-oci.lock` for a local flake reference; it is mandatory for a remote reference.

**Registry access and authentication** are delegated to `skopeo` (runtime dependency, wrapped into the `pin-oci` package and provided by the dev shells), which reads the host's own credentials (`REGISTRY_AUTH_FILE`, `${XDG_RUNTIME_DIR}/containers/auth.json`, `~/.docker/config.json`) and credential helpers. The CLI embeds no HTTP client and no credential handling, and does not depend on `virtualisation.oci-containers.backend`.


* `pin-oci lock --check [--flake <ref>] [--file <path>]` (there is no separate `check` subcommand):
Checks that every key of `lockTargets` has an entry for the platform of every host declaring it in the lockfile, without modifying any file. It evaluates the same `{ system; targets }` as `lock` (only the target keys are used), never the lockfile. Lockfile entries absent from `lockTargets` are ignored (only `lock` prunes them); a missing lockfile counts as empty.
Exit codes: 0 = in sync, 1 = out of sync, 2 = error (nix failure, unreadable or invalid lockfile, remote flake without `--file`). Suited for CI steps.

* `pin-oci lock --list [--flake <ref>] [--file <path>]`:
Lists every key that is declared (`lockTargets`) or locked, sorted, as a table `STATUS IMAGE HOSTS DIGEST`. `HOSTS` is the comma-separated list of hosts declaring the image (`-` for a stale one). Status is relative to the platforms of those hosts: `locked` (declared, has a digest for each of them), `missing` (declared, no entry for at least one, even if other systems are locked) or `stale` (locked, no longer declared). `DIGEST` shows, in full, one `<system> <digest>` per line for each of those platforms the lockfile has (every locked platform for a stale image). It evaluates the same `{ system; targets }` as `check`, reads the lockfile (absent = empty) and writes nothing; it takes no `Runtime`, so it cannot contact a registry. Exit codes: 0 on success whatever the statuses (use `--check` to gate CI), 2 on error. `--list` excludes `--check` and `--update`.

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
├── Cargo.lock
├── Cargo.toml
├── src/
│   ├── main.rs         # CLI entry point, clap subcommands
│   ├── check.rs        # `lock --check`: targets vs lockfile
│   ├── list.rs         # `lock --list`: declared and locked images with their status
│   ├── convert.rs      # `convert`: compose subset to a project, warnings and errors (pure)
│   ├── render.rs       # `convert`: project to Nix text, string escaping (pure)
│   ├── nix.rs          # nix eval invocation, system + lockTargets parsing
│   ├── lock.rs         # `lock` command: resolve missing, prune, write
│   ├── registry.rs     # Runtime trait around skopeo, index / single-manifest resolution
│   └── lockfile.rs     # BTreeMap model, JSON serialization
├── tests/              # Rust e2e (fake nix / skopeo on PATH), fixtures/ = captured skopeo outputs
│   └── nix/            # module tests: refs unit, fixture lockfiles, unit generation (flake check)
├── modules/
│   ├── default.nix     # Entry point aggregating the modules
│   ├── refs.nix        # Milestone 1: pure image reference normalization
│   ├── containers.nix  # Milestone 1: pin-oci engine + assertion
│   ├── infra.nix       # Milestone 2: systemd networks and volumes
│   └── projects.nix    # Milestone 3: Compose lowering to infra + containers
├── flake.nix           # inputs.nixpkgs, nixosModules.default, overlays.default, packages.pin-oci
└── README.md

```
