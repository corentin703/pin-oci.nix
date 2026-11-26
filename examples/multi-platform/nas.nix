# Only the NAS runs this one. `pin-oci lock` without `#<host>` locks the images of every host.
{
  virtualisation.pin-oci.containers.redis = {
    image = "redis:7-alpine";
    ports = [ "127.0.0.1:6379:6379" ];
  };
}
