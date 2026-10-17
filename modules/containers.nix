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

  pinContainerModule = import ./container-module.nix { inherit options; };

  unpinned = lib.filterAttrs (_: c: !(parseRef c.image).pinned) cfg.containers;

  wantsPull = c: if c.pullImage == null then cfg.pullImage else c.pullImage;

  # Consumed by the pin-oci CLI. MUST NOT depend on lockData.
  # `pullImage` is set when any container sharing the image pulls it into the store.
  lockTargets = lib.mapAttrs (
    key: group:
    let
      r = parseRef (builtins.head group).image;
    in
    {
      inherit (r) registry repository tag;
      pullImage = lib.any wantsPull group;
    }
  ) (lib.groupBy (c: (parseRef c.image).key) (lib.attrValues unpinned));

  isMissing =
    key:
    let
      entry = (lockData.images or { }).${key} or null;
    in
    entry == null || !((entry.arch or { }) ? ${currentSystem});

  missingKeys = lib.filter isMissing (builtins.attrNames lockTargets);
  formattedList = lib.concatMapStringsSep "\n  - " (k: k) missingKeys;

  pulledPinned = lib.filter (
    n: (parseRef cfg.containers.${n}.image).pinned && wantsPull cfg.containers.${n}
  ) (builtins.attrNames cfg.containers);

  # Locked images that must be pulled into the store but carry no hash for this platform.
  missingHashKeys = lib.filter (
    key: lockTargets.${key}.pullImage && !(lockData.images.${key}.arch.${currentSystem} ? nixHash)
  ) (lib.filter (key: !(lib.elem key missingKeys)) (builtins.attrNames lockTargets));

  # Replace the floating tag by the locked digest, or by the image fetched into the store.
  resolveContainer =
    _: containerCfg:
    let
      r = parseRef containerCfg.image;
      base = removeAttrs containerCfg [
        "_module"
        "pullImage"
      ];
      locked = lockData.images.${r.key}.arch.${currentSystem};
      name = "${r.registry}/${r.repository}";
    in
    if r.pinned then
      base
    else if wantsPull containerCfg then
      base
      // {
        image = "${name}:${r.tag}";
        imageFile = pkgs.dockerTools.pullImage {
          imageName = name;
          imageDigest = locked.digest;
          sha256 = locked.nixHash;
          finalImageName = name;
          finalImageTag = r.tag;
          inherit (locked) os;
          arch = locked.architecture;
        };
      }
    else
      base // { image = "${name}@${locked.digest}"; };
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

    pullImage = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Fetch every image into the Nix store at build time (`dockerTools.pullImage`) instead of
        letting the runtime pull it by digest. Overridable per container with `pullImage`.
        `pin-oci lock` then records the `nixHash` of each such image.
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
        {
          assertion = missingHashKeys == [ ];
          message = ''

            [pin-oci] Images pulled into the store have no nixHash for '${currentSystem}' in ${toString cfg.lockFile}:
              - ${lib.concatStringsSep "\n  - " missingHashKeys}

            Compute it with the command:
              pin-oci lock
          '';
        }
        {
          assertion = pulledPinned == [ ];
          message = ''

            [pin-oci] pullImage needs an image without a digest (pin-oci pins it); these already carry one:
              - ${lib.concatStringsSep "\n  - " pulledPinned}
          '';
        }
      ];

      virtualisation.oci-containers.containers =
        if missingKeys == [ ] && missingHashKeys == [ ] then
          lib.mapAttrs resolveContainer cfg.containers
        else
          { };
    })
  ];
}
