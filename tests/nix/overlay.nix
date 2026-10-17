{
  pkgs,
  nixpkgs,
  overlay,
  package,
}:

# `overlays.default` makes `pkgs.pin-oci` available. Pure: nothing is built.
let
  inherit (pkgs) lib;
  system = pkgs.stdenv.hostPlatform.system;
  extended = pkgs.extend overlay;

  # A host that opts in the way the README says, and uses the package from `pkgs`.
  host =
    (nixpkgs.lib.nixosSystem {
      inherit system;
      modules = [
        (
          { pkgs, ... }:
          {
            nixpkgs.overlays = [ overlay ];
            environment.systemPackages = [ pkgs.pin-oci ];
            system.stateVersion = "26.05";
            boot.loader.grub.devices = [ "nodev" ];
            fileSystems."/" = {
              device = "none";
              fsType = "tmpfs";
            };
          }
        )
      ];
    }).config;

  tests = {
    # The overlay hands out the flake's own package: same derivation, so the same cache hit.
    testOverlayProvidesThePackage = {
      expr = extended.pin-oci.drvPath;
      expected = package.drvPath;
    };
    testOverlayAddsOnlyPinOci = {
      expr = lib.attrNames (overlay extended pkgs);
      expected = [ "pin-oci" ];
    };
    testSystemPackagesResolvesThroughPkgs = {
      expr = lib.any (p: p.drvPath == package.drvPath) host.environment.systemPackages;
      expected = true;
    };
    # Falsification: nothing is injected without the overlay (option A, opt-in).
    testNotInPkgsWithoutOverlay = {
      expr = pkgs ? pin-oci;
      expected = false;
    };
  };
  failures = lib.debug.runTests tests;
in
pkgs.runCommand "pin-oci-overlay-tests" { } (
  if failures == [ ] then
    ''
      echo "${toString (builtins.length (builtins.attrNames tests))} overlay tests passed"
      touch $out
    ''
  else
    ''
      echo ${lib.escapeShellArg (builtins.toJSON failures)} >&2
      exit 1
    ''
)
