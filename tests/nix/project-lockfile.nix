{
  lib,
  system,
  eval,
  with',
  failed,
  failedByPinOci,
  lockOk,
  ...
}:

# `projects.<p>.followLockFile`: the images of a project are locked in the project's own lockfile,
# read as is; the host's lockfile and its `pin-oci lock` are not involved.
let
  lockProject = ./fixtures/project.lock;
  absentProject = ./fixtures/absent-project.lock; # intentionally does not exist

  pick = c: a: "sha256:${lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then a else c))}";
  hostDigest = pick "a" "c"; # caddy:2-alpine in ok.lock
  projectDigest = pick "1" "2"; # caddy:2-alpine in project.lock

  app =
    {
      lockFile ? lockProject,
      services ? {
        proxy.image = "caddy:2-alpine";
      },
    }:
    {
      virtualisation.pin-oci.projects.app = {
        enable = true;
        followLockFile = lockFile;
        inherit services;
      };
    };
  hostWeb.web.image = "caddy:2-alpine";

  evalApp =
    args:
    eval [
      (with' lockOk hostWeb)
      (app args)
    ];
  image = c: name: c.virtualisation.oci-containers.containers.${name}.image;
  targets = c: builtins.attrNames c.virtualisation.pin-oci.lockTargets;
  caddy = d: "docker.io/library/caddy@${d}";

  own = evalApp { };
in
{
  tests = {
    testProjectImageIsLockedInTheProjectLockfile = {
      expr = {
        project = image own "app-proxy";
        host = image own "web";
      };
      expected = {
        project = caddy projectDigest;
        host = caddy hostDigest;
      };
    };
    testNoFailedAssertion = {
      expr = failed own;
      expected = [ ];
    };
    # The host's `pin-oci lock` must not take over (or prune) what another flake locks.
    testProjectImagesAreNotLockTargets = {
      expr = targets (evalApp {
        services = {
          proxy.image = "caddy:2-alpine";
          cache.image = "redis:7";
        };
      });
      expected = [ "docker.io/library/caddy:2-alpine" ];
    };

    # ---- the host overrides it ----
    testNullMeansTheHostLockfile = {
      expr = image (evalApp { lockFile = null; }) "app-proxy";
      expected = caddy hostDigest;
    };
    testHostCanPointTheProjectAtItsOwnLockfile = {
      expr =
        let
          c = evalApp { lockFile = lockOk; };
        in
        {
          image = image c "app-proxy";
          targets = targets (evalApp {
            lockFile = lockOk;
            services.cache.image = "redis:7";
          });
        };
      expected = {
        image = caddy hostDigest;
        targets = [
          "docker.io/library/caddy:2-alpine"
          "docker.io/library/redis:7"
        ];
      };
    };
    testAnOverrideWinsOverTheFlakesDefault = {
      expr =
        let
          c = eval [
            (with' lockOk hostWeb)
            {
              virtualisation.pin-oci.projects.app = {
                enable = true;
                services.proxy.image = "caddy:2-alpine";
                followLockFile = lib.mkDefault lockProject;
              };
            }
            { virtualisation.pin-oci.projects.app.followLockFile = null; }
          ];
        in
        image c "app-proxy";
      expected = caddy hostDigest;
    };

    # ---- fail-fast, per lockfile ----
    testAnImageMissingFromTheProjectLockfileFails = {
      expr =
        let
          c = evalApp {
            services = {
              proxy.image = "caddy:2-alpine";
              cache.image = "redis:7";
            };
          };
        in
        map (a: lib.hasInfix "redis:7" a.message && lib.hasInfix "project.lock" a.message) (failed c);
      expected = [ true ];
    };
    testAnAbsentProjectLockfileFails = {
      expr = failedByPinOci (evalApp { lockFile = absentProject; });
      expected = [ true ];
    };
    testNothingIsLoweredWhileAProjectImageIsUnlocked = {
      expr =
        builtins.attrNames
          (evalApp { lockFile = absentProject; }).virtualisation.oci-containers.containers;
      expected = [ ];
    };

    # ---- backward compatibility ----
    testProjectWithoutLockFileIsLockedWithTheHost = {
      expr = {
        image = image (evalApp { lockFile = null; }) "app-proxy";
        targets = targets (evalApp { lockFile = null; });
      };
      expected = {
        image = caddy hostDigest;
        targets = [ "docker.io/library/caddy:2-alpine" ];
      };
    };
  };
}
