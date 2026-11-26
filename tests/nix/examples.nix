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
  multiLib = import (examples + "/rootless-multi/oci-lib.nix");
  multi = [
    (examples + "/rootless-multi/oci-users.nix")
    (examples + "/rootless-multi/containers.nix")
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

    # ---- rootless-multi: several projects, one user each, and the lib that derives them ----
    testExampleRootlessMultiPinsEveryProject = {
      expr = images (withLock lockExamples system multi);
      expected = {
        blog-db = pinned "library/postgres" system postgres;
        blog-app = pinned "miniflux/miniflux" system miniflux;
        tools-whoami = pinned "traefik/whoami" system whoami;
      };
    };
    testExampleRootlessMultiEachProjectRunsAsItsOwnUser = {
      expr =
        let
          c = withLock lockExamples system multi;
          svc = c.systemd.services;
        in
        {
          blogNetwork = svc.podman-network-blog-default.serviceConfig.User;
          toolsNetwork = svc.podman-network-tools-default.serviceConfig.User;
          blogXdg = svc.podman-network-blog-default.environment.XDG_RUNTIME_DIR;
          toolsXdg = svc.podman-network-tools-default.environment.XDG_RUNTIME_DIR;
          failed = failed c;
        };
      expected = {
        blogNetwork = "oci-blog";
        toolsNetwork = "oci-tools";
        blogXdg = "/run/user/2001";
        toolsXdg = "/run/user/2002";
        failed = [ ];
      };
    };
    # The users declared by oci-users.nix are the ones the projects name, with what pin-oci reads.
    testExampleRootlessMultiUsersMatchTheProjects = {
      expr =
        let
          c = withLock lockExamples system multi;
          u = n: c.users.users.${c.virtualisation.pin-oci.projects.${n}.podman.user};
        in
        lib.genAttrs [ "blog" "tools" ] (n: {
          inherit (u n) uid linger home;
        });
      expected = {
        blog = {
          uid = 2001;
          linger = true;
          home = "/srv/oci/blog/home";
        };
        tools = {
          uid = 2002;
          linger = true;
          home = "/srv/oci/tools/home";
        };
      };
    };
    # Falsifiable: a range that overlapped another one would give a container of one user the
    # files of another.
    testExampleRootlessMultiSubIdRangesDoNotOverlap = {
      expr =
        let
          c = withLock lockExamples system multi;
          range = n: lib.head c.users.users.${multiLib.userName n}.subUidRanges;
          a = range "blog";
          b = range "tools";
        in
        {
          disjoint = a.startUid + a.count <= b.startUid || b.startUid + b.count <= a.startUid;
          uidsAreUnique =
            lib.length (lib.unique (lib.attrValues multiLib.uids)) == lib.length (lib.attrValues multiLib.uids);
        };
      expected = {
        disjoint = true;
        uidsAreUnique = true;
      };
    };
    testExampleRootlessMultiHostId = {
      expr = {
        root = multiLib.hostId "blog" 0;
        first = multiLib.hostId "blog" 1;
        postgres = multiLib.hostId "blog" 70;
        otherUser = multiLib.hostId "tools" 1;
      };
      expected = {
        root = 2001;
        first = 1000000;
        postgres = 1000069;
        otherUser = 1065536;
      };
    };
    testExampleRootlessMultiDatabaseDirectoryBelongsToTheHostIdOfPostgres = {
      expr = lib.elem "d /srv/oci/blog/data/db 0700 1000069 1000069 -" (
        (withLock lockExamples system multi).systemd.tmpfiles.rules
      );
      expected = true;
    };
    # A container run by another user than the owner of the project network would not see it.
    testExampleRootlessMultiForeignUserFailsEvaluation = {
      expr =
        map
          (
            a: lib.hasInfix "tools-whoami (oci-blog) uses network 'tools-default' owned by oci-tools" a.message
          )
          (
            failed (
              withLock lockExamples system (
                multi ++ [ { virtualisation.pin-oci.projects.tools.services.whoami.podman.user = "oci-blog"; } ]
              )
            )
          );
      expected = [ true ];
    };
    testExampleRootlessMultiFailsWithoutALockfile = {
      expr = failedByPinOci (withLock lockAbsent system multi);
      expected = [ true ];
    };

    # ---- the lockfiles the examples ship: what `pin-oci lock` wrote, read as the examples do ----
    # No `withLock` here: each module's own `lockFile = ./pin-oci.lock` is the one under test. An
    # image declared by the example but missing from its lock would fail evaluation fast.
    testShippedLocksPinEveryImageOfTheirExample = {
      expr =
        let
          pinnedImages =
            sys: modules:
            let
              c = evalOn sys modules;
            in
            {
              failed = failed c;
              unpinned = lib.filter (i: !(lib.hasInfix "@sha256:" i)) (lib.attrValues (images c));
              count = lib.length (lib.attrNames (images c));
            };
        in
        {
          app = pinnedImages system app;
          rootless = pinnedImages system rootless;
          multi = pinnedImages system multi;
          server = pinnedImages x86 [ common ];
          nas = pinnedImages arm [
            common
            nas
          ];
        };
      expected =
        let
          ok = count: {
            failed = [ ];
            unpinned = [ ];
            inherit count;
          };
        in
        {
          app = ok 2;
          rootless = ok 2;
          multi = ok 3;
          server = ok 1;
          nas = ok 2;
        };
    };
    # Falsification: the shipped lock of one example is not enough for another one.
    testShippedLockOfMultiPlatformDoesNotCoverTheAppExample = {
      expr = failedByPinOci (
        evalOn system [
          (examples + "/app-with-database/containers.nix")
          { virtualisation.pin-oci.lockFile = lib.mkForce (examples + "/multi-platform/pin-oci.lock"); }
        ]
      );
      expected = [ true ];
    };
  };
}
