{
  lib,
  evalOn,
  system,
  failed,
  failedByPinOci,
  lockAbsent,
  ...
}:

# The modules under examples/ evaluate against a lockfile and lower to pinned containers, and
# fail fast without one. They are what the README and the flake template show, so they must not rot.
let
  lockExamples = ./fixtures/examples.lock;
  examples = ../../examples;

  # What `pin-oci lock` would have written in the example directory.
  withLock =
    lock: sys: modules:
    evalOn sys (modules ++ [ { virtualisation.pin-oci.lockFile = lib.mkForce lock; } ]);

  simple = [ (examples + "/simple/containers.nix") ];
  app = [ (examples + "/app-with-database/containers.nix") ];
  rootlessUsers = examples + "/rootless/users.nix";
  rootless = [
    rootlessUsers
    (examples + "/rootless/containers.nix")
  ];
  common = examples + "/multi-platform/common.nix";
  nas = examples + "/multi-platform/nas.nix";

  digest =
    sys: n:
    "sha256:${lib.concatStrings (lib.replicate 64 (if sys == "x86_64-linux" then n.x else n.a))}";
  images = c: lib.mapAttrs (_: x: x.image) c.virtualisation.oci-containers.containers;
  pinned =
    repo: sys: n:
    "docker.io/${repo}@${digest sys n}";

  # Same fixture rows as examples.lock: caddy 1/2, postgres 3/4, miniflux 5/6, redis 7/8.
  caddy = {
    x = "1";
    a = "2";
  };
  postgres = {
    x = "3";
    a = "4";
  };
  miniflux = {
    x = "5";
    a = "6";
  };
  redis = {
    x = "7";
    a = "8";
  };
  whoami = {
    x = "9";
    a = "a";
  };

  x86 = "x86_64-linux";
  arm = "aarch64-linux";

  # A lockfile that only knows x86_64: the aarch64 host of multi-platform must not evaluate.
  x86Only = builtins.toFile "x86-only.lock" (
    builtins.toJSON {
      version = 1;
      images."docker.io/library/caddy:2-alpine" = {
        registry = "docker.io";
        repository = "library/caddy";
        tag = "2-alpine";
        arch.${x86} = {
          digest = digest x86 caddy;
          os = "linux";
          architecture = "amd64";
        };
      };
    }
  );
in
{
  tests = {
    # A `follows` on nixpkgs makes Nix build the CLI against the user's nixpkgs: no binary cache.
    testExampleFlakesDoNotFollowNixpkgs = {
      expr =
        lib.mapAttrs (_: f: lib.hasInfix "follows =" (builtins.readFile (examples + "/${f}/flake.nix")))
          {
            simple = "simple";
            app = "app-with-database";
            multi = "multi-platform";
          };
      expected = {
        simple = false;
        app = false;
        multi = false;
      };
    };
    # ---- simple ----
    testExampleSimpleIsPinned = {
      expr = images (withLock lockExamples system simple);
      expected.caddy = pinned "library/caddy" system caddy;
    };
    testExampleSimpleFailsWithoutALockfile = {
      expr = failedByPinOci (withLock lockAbsent system simple);
      expected = [ true ];
    };

    # ---- app-with-database ----
    testExampleAppPinsBothServices = {
      expr = images (withLock lockExamples system app);
      expected = {
        miniflux-db = pinned "library/postgres" system postgres;
        miniflux-app = pinned "miniflux/miniflux" system miniflux;
      };
    };
    testExampleAppStartsTheAppAfterItsDatabase = {
      expr =
        let
          c = withLock lockExamples system app;
        in
        {
          after = lib.elem "podman-miniflux-db.service" c.systemd.services.podman-miniflux-app.after;
          requires = lib.elem "podman-miniflux-db.service" c.systemd.services.podman-miniflux-app.requires;
          volume = lib.elem "miniflux_db_data:/var/lib/postgresql/data" c.virtualisation.oci-containers.containers.miniflux-db.volumes;
        };
      expected = {
        after = true;
        requires = true;
        volume = true;
      };
    };
    testExampleAppFailsWithoutALockfile = {
      expr = failedByPinOci (withLock lockAbsent system app);
      expected = [ true ];
    };

    # ---- multi-platform: one lockfile, one digest per platform ----
    testExampleMultiPlatformServer = {
      expr = images (withLock lockExamples x86 [ common ]);
      expected.caddy = pinned "library/caddy" x86 caddy;
    };
    testExampleMultiPlatformNas = {
      expr = images (
        withLock lockExamples arm [
          common
          nas
        ]
      );
      expected = {
        caddy = pinned "library/caddy" arm caddy;
        redis = pinned "library/redis" arm redis;
      };
    };
    testExampleMultiPlatformMissingArchFailsFast = {
      expr = failedByPinOci (withLock x86Only arm [ common ]);
      expected = [ true ];
    };
    testExampleMultiPlatformServerIgnoresTheMissingArch = {
      expr = failed (withLock x86Only x86 [ common ]);
      expected = [ ];
    };

    # ---- rootless: the integrator's users.nix and the project that names its user ----
    testExampleRootlessPinsTheProject = {
      expr = images (withLock lockExamples system rootless);
      expected = {
        web-proxy = pinned "library/caddy" system caddy;
        web-whoami = pinned "traefik/whoami" system whoami;
      };
    };
    testExampleRootlessNetworkAndVolumeRunAsTheProjectUser = {
      expr =
        let
          c = withLock lockExamples system rootless;
        in
        {
          network = c.systemd.services.podman-network-web-default.serviceConfig.User;
          volume = c.systemd.services.podman-volume-web_caddy_data.serviceConfig.User;
          xdg = c.systemd.services.podman-network-web-default.environment.XDG_RUNTIME_DIR;
          failed = failed c;
        };
      expected = {
        network = "oci-web";
        volume = "oci-web";
        xdg = "/run/user/2001";
        failed = [ ];
      };
    };
    # users.nix and containers.nix are two halves: the user the project names is the one declared.
    testExampleRootlessUsersMatchTheProject = {
      expr =
        let
          c = withLock lockExamples system rootless;
          u = c.users.users.${c.virtualisation.pin-oci.projects.web.podman.user};
        in
        {
          inherit (u) uid linger;
          home = u.home;
        };
      expected = {
        uid = 2001;
        linger = true;
        home = "/srv/oci/web/home";
      };
    };
    testExampleRootlessFailsWithoutALockfile = {
      expr = failedByPinOci (withLock lockAbsent system rootless);
      expected = [ true ];
    };
  };
}
