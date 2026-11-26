# A reverse proxy and a backend, as one project that a single rootless user manages.
#
# Before pin-oci could do it, the network of such a project had to be wired by hand to the user's
# Podman (`User`, `HOME`, `XDG_RUNTIME_DIR`, `after` on `user@<uid>.service`, `/run/wrappers`).
# Now `podman.user` says which user manages what, for containers, networks and volumes alike.
{
  virtualisation.oci-containers.backend = "podman"; # `podman.user` is for Podman only

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    projects.web = {
      enable = true;

      # The user of the project: the default owner of its services, networks and volumes.
      # Each of them can set its own `podman.user`; a container that is not run by the owner of
      # a network or volume it uses fails the evaluation, because it would not see it.
      podman.user = "oci-web";

      networks.default.enable = true; # `whoami` resolves by its name inside the project
      volumes.caddy_data = { };

      services = {
        proxy = {
          image = "caddy:2-alpine";
          dependsOn = [ "whoami" ];
          ports = [
            "8080:80"
            "8443:443"
          ];
          volumes = [
            "caddy_data:/data"
            # A directory of the user's workspace (see users.nix).
            "/srv/oci/web/data:/srv:ro"
          ];
        };

        whoami.image = "traefik/whoami:v1.10";
      };
    };

    # A resource outside a project says the same thing:
    #   networks.shared.podman.user = "oci-web";
    #   volumes.shared_data.podman.user = "oci-web";
  };
}
