{
  virtualisation.oci-containers.backend = "podman"; # or "docker"

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    containers.caddy = {
      image = "caddy:2-alpine"; # a floating tag: pin-oci lock turns it into a digest
      ports = [
        "80:80"
        "443:443"
      ];
      volumes = [ "/srv/caddy/data:/data" ];
    };
  };
}
