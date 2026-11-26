# Two projects, each run by its own rootless user (see oci-lib.nix and oci-users.nix):
#   blog  - Miniflux and its PostgreSQL database;
#   tools - a service that has nothing to do with the blog, and must not see its network.
#
# `podman.user` is the owner of a project's services, networks and volumes. Two projects with two
# users share nothing: not the images, not the networks, not the files.
let
  oci = import ./oci-lib.nix;

  # `postgres` is uid 70 in the Alpine image. Seen from the host (rootless) it is not 70.
  postgres = toString (oci.hostId "blog" 70);
in
{
  virtualisation.oci-containers.backend = "podman"; # `podman.user` is for Podman only

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    projects.blog = {
      enable = true;
      podman.user = oci.userName "blog";
      networks.default.enable = true; # the app reaches its database by name: `db`

      services = {
        db = {
          image = "postgres:17-alpine";
          environment = {
            POSTGRES_USER = "miniflux";
            POSTGRES_DB = "miniflux";
          };
          # POSTGRES_PASSWORD=... Keep secrets out of the Nix store.
          environmentFiles = [ "/run/secrets/blog-db.env" ];
          volumes = [ "${oci.dataDir "blog"}/db:/var/lib/postgresql/data" ];
        };

        app = {
          image = "miniflux/miniflux:latest";
          dependsOn = [ "db" ];
          ports = [ "8080:8080" ];
          environment.RUN_MIGRATIONS = "1";
          # DATABASE_URL=postgres://miniflux:<password>@db/miniflux?sslmode=disable
          environmentFiles = [ "/run/secrets/blog-app.env" ];
        };
      };
    };

    projects.tools = {
      enable = true;
      podman.user = oci.userName "tools";
      networks.default.enable = true;

      services.whoami = {
        image = "traefik/whoami:v1.10";
        ports = [ "8081:80" ];
      };
    };

    # A container of `tools` that used the network of `blog` would fail the evaluation: it runs as
    # another user, so it would not see it.
  };

  # The database writes as `postgres`: its directory belongs to the host id of that user.
  systemd.tmpfiles.rules = [ "d ${oci.dataDir "blog"}/db 0700 ${postgres} ${postgres} -" ];
}
