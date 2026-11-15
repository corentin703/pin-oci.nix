# Same options as oci-containers, derived from the host configuration, plus `pullImage`.
{ options }:
{ lib, ... }:
{
  imports = options.virtualisation.oci-containers.containers.type.nestedTypes.elemType.getSubModules;

  options.pullImage = lib.mkOption {
    type = lib.types.nullOr lib.types.bool;
    default = null;
    example = true;
    description = ''
      Fetch the image into the Nix store at build time (`dockerTools.pullImage`) instead of
      letting the runtime pull it. `null` follows `virtualisation.pin-oci.pullImage`.
      Needs the `nixHash` that `pin-oci lock` records for such images.
    '';
  };

  # Set by `projects.<name>.lockFile`; never by hand. `null`: the host's lockFile.
  options.lockFile = lib.mkOption {
    type = lib.types.nullOr lib.types.path;
    default = null;
    internal = true;
    visible = false;
    description = "Lockfile this container's image is locked in.";
  };
}
