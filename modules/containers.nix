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
