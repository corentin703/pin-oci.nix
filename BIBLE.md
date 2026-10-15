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

Consumption as a flake input; `follows` keeps the module aligned with the host's nixpkgs:

```nix
inputs = {
  nixpkgs.url = "github:nixos/nixpkgs/nixos-26.05";

  pin-oci = {
    url = "github:cprag/pin-oci.nix";
    inputs.nixpkgs.follows = "nixpkgs";
  };
};
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

### 4.2 Nix Module (`modules/containers.nix`)

```nix
{ config, options, lib, pkgs, ... }:

let
  cfg = config.virtualisation.pin-oci;
  lockFilePath = cfg.lockFile;

  lockData =
    if builtins.pathExists lockFilePath then
      builtins.fromJSON (builtins.readFile lockFilePath)
    else
      { version = 1; images = { }; };

  currentSystem = pkgs.stdenv.hostPlatform.system;

  # Options derived from the host's oci-containers, never from <nixpkgs>.
  # Exact derivation mechanism validated at implementation time (task 1.6).
  baseContainerModules =
    options.virtualisation.oci-containers.containers.type.nestedTypes.elemType.getSubModules;

  pinContainerModule = { imports = baseContainerModules; };

  # Canonical reference (see §2). Only place where references are parsed.
  parseRef = ref:
    let
      pinned = lib.hasInfix "@" ref;
      noDigest = builtins.head (lib.splitString "@" ref);
      parts = lib.splitString "/" noDigest;
      first = builtins.head parts;
      hasRegistry =
        builtins.length parts > 1
        && (lib.hasInfix "." first || lib.hasInfix ":" first || first == "localhost");
      registry = if hasRegistry then first else "docker.io";
      path = if hasRegistry then lib.concatStringsSep "/" (builtins.tail parts) else noDigest;
      lastSegment = lib.last (lib.splitString "/" path);
      hasTag = lib.hasInfix ":" lastSegment;
      tag = if hasTag then lib.last (lib.splitString ":" lastSegment) else "latest";
      untagged = if hasTag then lib.removeSuffix ":${tag}" path else path;
      repository =
        if registry == "docker.io" && !(lib.hasInfix "/" untagged)
        then "library/${untagged}"
        else untagged;
    in
    { inherit pinned registry repository tag; key = "${registry}/${repository}:${tag}"; };

  unpinned = lib.filterAttrs (_: c: !(parseRef c.image).pinned) cfg.containers;

  # Targets consumed by the CLI. MUST NOT depend on lockData.
  lockTargets = lib.mapAttrs'
    (_: c:
      let r = parseRef c.image;
      in lib.nameValuePair r.key { inherit (r) registry repository tag; })
    unpinned;

  isMissing = key:
    let entry = lockData.images.${key} or null;
    in entry == null || !(entry.arch ? ${currentSystem});

  missingKeys = lib.filter isMissing (builtins.attrNames lockTargets);
  formattedList = lib.concatMapStringsSep "\n  - " (k: k) missingKeys;

  # Image resolution: replace :tag with @sha256:...
  resolveContainer = _: containerCfg:
    let
      r = parseRef containerCfg.image;
    in
    if r.pinned then containerCfg
    else
      containerCfg // {
        image = "${r.registry}/${r.repository}@${lockData.images.${r.key}.arch.${currentSystem}.digest}";
      };

in {
  options.virtualisation.pin-oci = {
    enable = lib.mkEnableOption "pin-oci deterministic container manager";

    lockFile = lib.mkOption {
      type = lib.types.path;
      description = ''
        Path to pin-oci.lock, at the flake root next to flake.lock.
        No default: set it to ./pin-oci.lock or (self + "/pin-oci.lock").
      '';
    };

    containers = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule pinContainerModule);
      default = { };
      description = "Container specifications (identical to oci-containers).";
    };

    lockTargets = lib.mkOption {
      type = lib.types.attrsOf lib.types.attrs;
      readOnly = true;
      description = "Canonical image keys to resolve, consumed by the pin-oci CLI.";
    };
  };

  config = lib.mkMerge [
    { virtualisation.pin-oci.lockTargets = lockTargets; }

    (lib.mkIf cfg.enable {
      assertions = [
        {
          assertion = missingKeys == [ ];
          message = ''

            [pin-oci] Missing or unpinned images for '${currentSystem}' in ${toString lockFilePath}:
              - ${formattedList}

            Synchronize the lock file with the command:
              pin-oci lock
            then make it visible to the flake:
              git add pin-oci.lock
          '';
        }
      ];

      virtualisation.oci-containers.containers =
        if missingKeys == [ ] then
          lib.mapAttrs resolveContainer cfg.containers
        else
          { };
    })
  ];
}

```

**Laziness guarantee.** Assertions are only forced by `system.build.toplevel`. `lockTargets` does not depend on `lockData`, so `nix eval ...lockTargets` succeeds with a missing lockfile, while `nixos-rebuild` fails with the message above.

### 4.3 Rust CLI Specification (`pin-oci`)

The CLI never parses image references and holds no registry client or credentials: it asks Nix for the canonical targets and delegates registry access and authentication to `skopeo`, using the host's own credentials and credential helpers.

#### Commands:

* `pin-oci lock [--flake <ref>] [--file <path>]`:
1. Evaluates the configuration without building, retrieving the host system and the targets in a single evaluation:
```bash
nix eval --json <flake>#nixosConfigurations."<host>".config \
  --apply 'c: { system = c.nixpkgs.hostPlatform.system; targets = c.virtualisation.pin-oci.lockTargets; }'

```


2. Resolves only the targets missing from the lockfile (absent key, or no entry for the host system); existing entries are never re-resolved.
3. For each, runs `skopeo inspect --raw docker://<registry>/<repository>:<tag>`:
   * **index** (`manifests[]`): takes each entry's `digest` and `platform.{os,architecture,variant}`, maps platforms through the table of §2 and ignores unmappable entries (`unknown/unknown` attestations, `s390x`, `arm` v6...);
   * **single manifest** (no `manifests`): runs `skopeo inspect --no-tags` and reads `Digest`, `Os`, `Architecture`, `Variant`; an unmappable platform is an error.
4. Fails if an image has no manifest for the host system, or if two manifests map to the same system.
5. Prunes lockfile entries absent from `lockTargets`.
6. Writes deterministic JSON (`BTreeMap` order, fixed field order, 2-space indent, final newline), atomically (temporary file then rename), and leaves the file untouched when the content is unchanged.

**All or nothing:** if any image fails to resolve, every failure is reported, nothing is written and the exit code is 2.

`--file` defaults to `<flake root>/pin-oci.lock` for a local flake reference; it is mandatory for a remote reference.

**Registry access and authentication** are delegated to `skopeo` (runtime dependency, wrapped into the `pin-oci` package and provided by the dev shells), which reads the host's own credentials (`REGISTRY_AUTH_FILE`, `${XDG_RUNTIME_DIR}/containers/auth.json`, `~/.docker/config.json`) and credential helpers. The CLI embeds no HTTP client and no credential handling, and does not depend on `virtualisation.oci-containers.backend`.


* `pin-oci check [--flake <ref>] [--file <path>]`:
Checks that every key of `lockTargets` has an entry for the host platform in the lockfile, without modifying any file. It evaluates the same `{ system; targets }` as `lock` (only the target keys are used), never the lockfile. Lockfile entries absent from `lockTargets` are ignored (only `lock` prunes them); a missing lockfile counts as empty.
Exit codes: 0 = in sync, 1 = out of sync, 2 = error (nix failure, unreadable or invalid lockfile, remote flake without `--file`). Suited for CI steps.

`--flake` takes `<flake>#<host>` and targets `nixosConfigurations."<host>".config`; a reference without `#<host>` is an error.

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

For each defined resource, a `oneshot` systemd unit is generated:

* **Network:** `${backend}-network-${name}.service`
```bash
ExecStart = "${pkgs.${backend}}/bin/${backend} network exists ${name} || ${pkgs.${backend}}/bin/${backend} network create ${name} [flags]"

```


* **Volume:** `${backend}-volume-${name}.service`
```bash
ExecStart = "${pkgs.${backend}}/bin/${backend} volume exists ${name} || ${pkgs.${backend}}/bin/${backend} volume create ${name} [flags]"

```


* **Container ordering:**
The `${backend}-${container}.service` units automatically receive:
```nix
after = [ "${backend}-network-app-net.service" "${backend}-volume-postgres_data.service" ];
requires = [ "${backend}-network-app-net.service" "${backend}-volume-postgres_data.service" ];

```



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

1. **Name prefixing and `external`:**
* Standardized container: `<project>-<service>` (e.g. `monitoring-grafana`).
* Project `volumes.<v>` and `networks.<n>` carry an `external` flag (default `false`).
* `external = false`: the resource is prefixed with the project (volume `<project>_<volume>`, e.g. `monitoring_grafana_data`; network `<project>-<network>`) and its init unit is generated. Volume references in the project's services (`grafana_data:/var/lib/grafana`) are rewritten to the prefixed name.
* `external = true`: no prefix, no unit generated; an assertion requires the resource to exist in the global `virtualisation.pin-oci.{networks,volumes}`.


2. **Local DNS resolution:**
`networks.default.enable = true` creates the `<project>-default` network (never external) and attaches it to all services of the project. Podman/Docker handles internal DNS resolution via the service name (`prometheus`).
3. **`dependsOn` translation:**
Injection of systemd dependencies on the target container:
```nix
after = [ "${backend}-${project}-${depService}.service" ];
requires = [ "${backend}-${project}-${depService}.service" ];

```


4. **Feeding the CLI:**
`virtualisation.pin-oci.lockTargets` aggregates the images from `config.virtualisation.pin-oci.containers` and from all `config.virtualisation.pin-oci.projects.*.services`; `pin-oci lock` consumes that single option.

---

## 7. Git Repository Layout

```text
pin-oci.nix/
├── Cargo.lock
├── Cargo.toml
├── src/
│   ├── main.rs         # CLI entry point, clap subcommands
│   ├── check.rs        # `check` command: targets vs lockfile
│   ├── nix.rs          # nix eval invocation, system + lockTargets parsing
│   ├── lock.rs         # `lock` command: resolve missing, prune, write
│   ├── registry.rs     # Runtime trait around skopeo, index / single-manifest resolution
│   └── lockfile.rs     # BTreeMap model, JSON serialization
├── tests/              # e2e (fake nix / skopeo on PATH), fixtures/ = captured skopeo outputs
├── modules/
│   ├── default.nix     # Entry point aggregating the modules
│   ├── containers.nix  # Milestone 1: pin-oci engine + assertion
│   ├── infra.nix       # Milestone 2: systemd networks and volumes
│   └── projects.nix    # Milestone 3: Compose lowering to infra + containers
├── flake.nix           # inputs.nixpkgs, nixosModules.default, packages.pin-oci
└── README.md

```
