# Evaluated by the CLI on `nixosConfigurations."<host>".config` (`nix eval --apply`).
# Single source of truth: also imported by the Nix module tests.
c: {
  system = c.virtualisation.pin-oci.system;
  targets = c.virtualisation.pin-oci.lockTargets;
}
