# Miniflux and its PostgreSQL database, declared as one project: containers
# `miniflux-app` and `miniflux-db`, volume `miniflux_db_data`, and the target
# `podman-miniflux.target` to restart both at once.
{
  virtualisation.oci-containers.backend = "podman"; # or "docker"

  virtualisation.pin-oci = {
    enable = true;
    lockFile = ./pin-oci.lock;

    projects.miniflux = {
      enable = true;
      networks.default.enable = true; # services reach each other by name: `db`

      services = {
        db = {
          image = "postgres:17-alpine";
          environment = {
            POSTGRES_USER = "miniflux";
            POSTGRES_DB = "miniflux";
          };
          # POSTGRES_PASSWORD=... Keep secrets out of the Nix store.
          environmentFiles = [ "/run/secrets/miniflux-db.env" ];
          volumes = [ "db_data:/var/lib/postgresql/data" ];
        };

        app = {
          image = "miniflux/miniflux:2.2";
          dependsOn = [ "db" ];
          ports = [ "8080:8080" ];
          environment.RUN_MIGRATIONS = "1";
          # DATABASE_URL=postgres://miniflux:<password>@db/miniflux?sslmode=disable
          environmentFiles = [ "/run/secrets/miniflux-app.env" ];
        };
      };

      volumes.db_data = { };
    };
  };
}
