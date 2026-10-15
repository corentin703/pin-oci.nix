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

Default location: root of the flake / Nix configuration (`./pin-oci.lock`).

```json
{
  "version": 1,
  "images": {
    "caddy:2-alpine": {
      "registry": "docker.io",
      "repository": "library/caddy",
      "tag": "2-alpine",
      "manifest_digest": "sha256:4a3b890123...",
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

The submodule replicates the exact options of `virtualisation.oci-containers.containers.<name>`, except for the `image` field, which receives an unresolved string (`"image:tag"`).

```nix
virtualisation.pin-oci = {
  enable = true;
  lockFile = ./pin-oci.lock; # optional, default: ./pin-oci.lock

  containers.traefik = {
    image = "traefik:v3.0";
    ports = [ "80:80" "443:443" ];
    volumes = [ "/var/run/docker.sock:/var/run/docker.sock:ro" ];
  };
};

```

### 4.2 Nix Module (`modules/pin-oci.nix`)

```nix
{ config, lib, pkgs, ... }:

let
  cfg = config.virtualisation.pin-oci;
  lockFilePath = cfg.lockFile;

  lockData =
    if builtins.pathExists lockFilePath then
      builtins.fromJSON (builtins.readFile lockFilePath)
    else
      { version = 1; images = { }; };

  currentSystem = pkgs.stdenv.hostPlatform.system;

  # Strict inheritance of the oci-containers options
  baseContainerModule = import <nixpkgs/nixos/modules/virtualisation/oci-options.nix> { inherit lib; };

  pinContainerModule = {
    imports = [ baseContainerModule ];
    options.image = lib.mkOption {
      type = lib.types.str;
      description = "OCI reference with a floating tag (e.g. redis:7-alpine).";
    };
  };

  # Check for presence in the lockfile
  isImageMissing = imageTag:
    let
      entry = lockData.images.${imageTag} or null;
    in
    entry == null || !(entry.arch ? ${currentSystem});

  missingContainers = lib.filterAttrs (_: c: isImageMissing c.image) cfg.containers;
  missingImagesList = lib.unique (lib.mapAttrsToList (_: c: c.image) missingContainers);
  formattedList = lib.concatMapStringsSep "\n  - " (img: img) missingImagesList;

  # Image resolution: replace :tag with @sha256:...
  resolveContainer = _: containerCfg:
    let
      rawImage = containerCfg.image;
      entry = lockData.images.${rawImage};
      digest = entry.arch.${currentSystem}.digest;
      baseImage = builtins.head (lib.splitString ":" (builtins.head (lib.splitString "@" rawImage)));
    in
    containerCfg // {
      image = "${baseImage}@${digest}";
    };

in {
  options.virtualisation.pin-oci = {
    enable = lib.mkEnableOption "pin-oci deterministic container manager";

    lockFile = lib.mkOption {
      type = lib.types.path;
      default = ./pin-oci.lock;
      description = "Absolute or relative path to pin-oci.lock.";
    };

    containers = lib.mkOption {
      type = lib.types.attrsOf (lib.types.submodule pinContainerModule);
      default = { };
      description = "Container specifications (identical to oci-containers).";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = missingImagesList == [ ];
        message = ''

          [pin-oci] Missing or unpinned images for '${currentSystem}' in ${toString lockFilePath}:
            - ${formattedList}

          Synchronize the lock file with the command:
            pin-oci lock
        '';
      }
    ];

    virtualisation.oci-containers.containers =
      if missingImagesList == [ ] then
        lib.mapAttrs resolveContainer cfg.containers
      else
        { };
  };
}

```

### 4.3 Rust CLI Specification (`pin-oci`)

#### Commands:

* `pin-oci lock [--flake <ref>] [--file <path>]`:
1. Evaluates the configuration without building:
```bash
nix eval --json <ref>.config.virtualisation.pin-oci.containers --apply 'builtins.mapAttrs (_: c: c.image)'

```


2. Parses the image references (splitting registry, repository, tag).
3. Performs OCI v2 Registry requests (`GET /v2/<name>/manifests/<tag>`) with accept headers for `image.index.v1` and `manifest.list.v2`.
4. Extracts the individual per-platform digests (`amd64`, `arm64`, etc.).
5. Writes deterministically sorted JSON (`BTreeMap`) to the target file.


* `pin-oci check [--flake <ref>]`:
Checks that all declared images are present in the lockfile without modifying any files (exit code 0 = in sync, 1 = out of sync; suited for CI steps).

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
  defaultNetwork.enable = true; # Automatically creates the 'monitoring-default' network

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

```

### 6.2 Compilation Rules to Milestones 1 & 2

1. **Name prefixing:**
* Standardized container: `<project>-<service>` (e.g. `monitoring-grafana`).
* Standardized volume: `<project>_<volume>` (e.g. `monitoring_grafana_data`).


2. **Local DNS resolution:**
The implicit `<project>-default` network connects all services of the project. Podman/Docker handles internal DNS resolution via the service name (`prometheus`).
3. **`dependsOn` translation:**
Injection of systemd dependencies on the target container:
```nix
after = [ "${backend}-${project}-${depService}.service" ];
wants = [ "${backend}-${project}-${depService}.service" ];

```


4. **Feeding the CLI:**
The `pin-oci lock` command recursively aggregates the images from `config.virtualisation.pin-oci.containers` and from all `config.virtualisation.pin-oci.projects.*.services`.

---

## 7. Git Repository Layout

```text
pin-oci.nix/
├── Cargo.lock
├── Cargo.toml
├── src/
│   ├── main.rs         # CLI entry point, clap subcommands
│   ├── nix.rs          # nix eval invocation, target parsing
│   ├── registry.rs     # OCI v2 client, manifest negotiation
│   └── lockfile.rs     # BTreeMap model, JSON serialization
├── modules/
│   ├── default.nix     # Entry point aggregating the modules
│   ├── containers.nix  # Milestone 1: pin-oci engine + assertion
│   ├── infra.nix       # Milestone 2: systemd networks and volumes
│   └── projects.nix    # Milestone 3: Compose lowering to infra + containers
├── flake.nix
└── README.md

```
