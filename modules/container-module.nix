# Same options as oci-containers, derived from the host configuration.
{ options }:
{
  imports = options.virtualisation.oci-containers.containers.type.nestedTypes.elemType.getSubModules;
}
