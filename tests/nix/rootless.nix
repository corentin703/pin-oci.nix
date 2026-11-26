{
  lib,
  eval,
  with',
  failed,
  failedByPinOci,
  lockOk,
  ...
}:

# `podman.user` on networks and volumes: the unit runs as that user, reading the account from
# `users.users`; pin-oci declares no user. Everything else stays as without it.
let
  base = with' lockOk { };

  # What an integrator declares himself: pin-oci only reads uid, home and linger.
  account =
    {
      name ? "oci-app",
      uid ? 2001,
      linger ? true,
    }:
    {
      users.groups.${name} = { };
      users.users.${name} = {
        isSystemUser = true;
        inherit uid linger;
        group = name;
        home = "/srv/oci/${name}/home";
      };
    };

  resources =
    {
      owner ? "oci-app",
    }:
    {
      virtualisation.pin-oci.networks.app-net = {
        subnet = "10.89.1.0/24";
        podman.user = owner;
      };
      virtualisation.pin-oci.volumes.app_data.podman.user = owner;
    };

  container =
    {
      user,
      extra ? { },
    }:
    {
      virtualisation.pin-oci.containers.web = {
        image = "caddy:2-alpine";
        podman.user = user;
        networks = [ "app-net" ];
        volumes = [ "app_data:/data" ];
      }
      // extra;
    };

  ok = eval [
    base
    (account { })
    (resources { })
    (container { user = "oci-app"; })
  ];
  unit = c: name: c.systemd.services.${name};
  net = unit ok "podman-network-app-net";
  vol = unit ok "podman-volume-app_data";

  rootDefault = eval [
    base
    (resources { owner = "root"; })
  ];
  rootNet = unit rootDefault "podman-network-app-net";

  project =
    {
      user,
      service ? { },
      volume ? { },
    }:
    {
      virtualisation.pin-oci.projects.app = {
        enable = true;
        podman.user = user;
        networks.default.enable = true;
        volumes.data = volume;
        services.web = {
          image = "caddy:2-alpine";
          volumes = [ "data:/data" ];
        }
        // service;
      };
    };
  owned = c: {
    container = c.virtualisation.pin-oci.containers.app-web.podman.user;
    network = c.virtualisation.pin-oci.networks.app-default.podman.user;
    volume = c.virtualisation.pin-oci.volumes.app_data.podman.user;
  };
  evalProject =
    args:
    eval [
      base
      (account { })
      (account {
        name = "oci-other";
        uid = 2002;
      })
      (project args)
    ];

  asserts = modules: failedByPinOci (eval ([ base ] ++ modules));
  docker = {
    virtualisation.oci-containers.backend = lib.mkForce "docker";
  };
in
{
  tests = {
    # ---- the unit of a resource owned by a user ----
    testNetworkRunsAsItsOwner = {
      expr = {
        user = net.serviceConfig.User;
        inherit (net.serviceConfig) Type RemainAfterExit;
        env = {
          inherit (net.environment) HOME XDG_RUNTIME_DIR;
        };
        wrappers = lib.elem "/run/wrappers" net.path;
        after = lib.elem "user@2001.service" net.after;
        requires = lib.elem "user@2001.service" net.requires;
        linger = lib.elem "linger-users.service" net.after && lib.elem "linger-users.service" net.wants;
      };
      expected = {
        user = "oci-app";
        Type = "oneshot";
        RemainAfterExit = true;
        env = {
          HOME = "/srv/oci/oci-app/home";
          XDG_RUNTIME_DIR = "/run/user/2001";
        };
        wrappers = true;
        after = true;
        requires = true;
        linger = true;
      };
    };
    testVolumeRunsAsItsOwner = {
      expr = {
        user = vol.serviceConfig.User;
        env = vol.environment.XDG_RUNTIME_DIR;
        after = lib.elem "user@2001.service" vol.after;
      };
      expected = {
        user = "oci-app";
        env = "/run/user/2001";
        after = true;
      };
    };
    # The body is the one of root's unit: still idempotent, still the same flags.
    testOwnedUnitKeepsItsIdempotentBody = {
      expr = {
        checksFirst = lib.hasInfix "/bin/podman network inspect app-net >/dev/null 2>&1" net.script;
        creates = lib.hasInfix "/bin/podman network create --subnet 10.89.1.0/24 app-net" net.script;
        wantedBy = net.wantedBy;
      };
      expected = {
        checksFirst = true;
        creates = true;
        wantedBy = [ "multi-user.target" ];
      };
    };
    testOwnerWithoutLingerIsNotOrderedAfterIt = {
      expr =
        let
          c = eval [
            base
            (account { linger = false; })
            (resources { })
          ];
          n = unit c "podman-network-app-net";
        in
        {
          after = lib.elem "linger-users.service" n.after;
          wants = lib.elem "linger-users.service" n.wants;
          warned = lib.any (lib.hasInfix "lingering is turned off") c.warnings;
        };
      expected = {
        after = false;
        wants = false;
        warned = true;
      };
    };

    # ---- nothing changes for root ----
    testRootStaysTheDefault = {
      expr = {
        hasUser = rootNet.serviceConfig ? User;
        env = lib.filterAttrs (n: _: n == "HOME" || n == "XDG_RUNTIME_DIR") rootNet.environment;
        wants = rootNet.wants;
        wrappers = lib.elem "/run/wrappers" rootNet.path;
        failed = failed rootDefault;
        warnings = lib.filter (lib.hasInfix "[pin-oci]") rootDefault.warnings;
      };
      expected = {
        hasUser = false;
        env = { };
        wants = [ ];
        wrappers = false;
        failed = [ ];
        warnings = [ ];
      };
    };

    # ---- guarantees that must be able to fail ----
    testSameOwnerPasses = {
      expr = failed ok;
      expected = [ ];
    };
    testContainerOfAnotherUserIsRefused = {
      expr = asserts [
        (account { })
        (account {
          name = "oci-other";
          uid = 2002;
        })
        (resources { })
        (container { user = "oci-other"; })
      ];
      expected = [ true ];
    };
    testRootContainerOnAnOwnedResourceIsRefused = {
      expr = asserts [
        (account { })
        (resources { })
        (container { user = "root"; })
      ];
      expected = [ true ];
    };
    testRootlessContainerOnARootResourceIsRefused = {
      expr = asserts [
        (account { })
        (resources { owner = "root"; })
        (container { user = "oci-app"; })
      ];
      expected = [ true ];
    };
    testUserWithoutUidIsRefused = {
      expr = asserts [
        {
          users.groups.oci-app = { };
          users.users.oci-app = {
            isSystemUser = true;
            group = "oci-app";
          };
        }
        (resources { })
      ];
      expected = [ true ];
    };
    testUndeclaredUserIsRefusedNotAnEvaluationError = {
      expr = asserts [ (resources { owner = "nobody-declared"; }) ];
      expected = [ true ];
    };
    testDockerBackendRefusesAnOwner = {
      expr = asserts [
        docker
        (account { })
        (resources { })
      ];
      expected = [ true ];
    };
    testDockerStaysFineWithRoot = {
      expr = failed (eval [
        base
        docker
        (resources { owner = "root"; })
      ]);
      expected = [ ];
    };
    testOwnedResourceNotReferencedIsFine = {
      expr = failed (eval [
        base
        (account { })
        (resources { })
      ]);
      expected = [ ];
    };

    # ---- projects: one line says which user manages the project ----
    testProjectUserIsTheDefaultOfEverythingItCreates = {
      expr = owned (evalProject {
        user = "oci-app";
      });
      expected = {
        container = "oci-app";
        network = "oci-app";
        volume = "oci-app";
      };
    };
    testProjectResourceUnitsRunAsTheProjectUser = {
      expr =
        let
          c = evalProject { user = "oci-app"; };
        in
        {
          network = c.systemd.services.podman-network-app-default.serviceConfig.User;
          volume = c.systemd.services.podman-volume-app_data.serviceConfig.User;
          failed = failed c;
        };
      expected = {
        network = "oci-app";
        volume = "oci-app";
        failed = [ ];
      };
    };
    testProjectWithoutUserStaysRoot = {
      expr = owned (eval [
        base
        (project { user = null; })
      ]);
      expected = {
        container = "root";
        network = "root";
        volume = "root";
      };
    };
    testServiceCanOverrideTheProjectUser = {
      expr =
        (evalProject {
          user = "oci-app";
          service.podman.user = "oci-other";
        }).virtualisation.pin-oci.containers.app-web.podman.user;
      expected = "oci-other";
    };
    # An override that leaves a service apart from the project's resources is the §5.3 assertion.
    testServiceOverrideAwayFromItsResourcesIsRefused = {
      expr = failedByPinOci (evalProject {
        user = "oci-app";
        service.podman.user = "oci-other";
      });
      expected = [ true ];
    };
    testResourceOverrideAwayFromItsServicesIsRefused = {
      expr = failedByPinOci (evalProject {
        user = "oci-app";
        volume.podman.user = "oci-other";
      });
      expected = [ true ];
    };
    testResourceCanOverrideTheProjectUser = {
      expr =
        (evalProject {
          user = "oci-app";
          volume.podman.user = "oci-other";
        }).virtualisation.pin-oci.volumes.app_data.podman.user;
      expected = "oci-other";
    };
  };
}
