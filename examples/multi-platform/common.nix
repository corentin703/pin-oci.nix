# Shared by every host. The lockfile holds one digest per platform for each image, so the
# x86_64 server and the aarch64 NAS read the same pin-oci.lock.
{
  virtualisation.oci-containers.backend = "podman";

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    containers.caddy = {
      image = "caddy:2-alpine";
      ports = [
        "80:80"
        "443:443"
      ];
    };
  };
}
