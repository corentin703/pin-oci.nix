# Evaluated by the CLI on every `nixosConfigurations."<host>".config` when no host is given.
# Same shape as apply.nix, or null for a host that does not use pin-oci.
c:
let
  p = c.virtualisation.pin-oci or null;
in
if p != null && p.enable then
  {
    system = p.system;
    targets = p.lockTargets;
  }
else
  null
