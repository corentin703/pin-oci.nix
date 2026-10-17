{
  lib,
  system,
  eval,
  with',
  failed,
  failedByPinOci,
  lockOk,
  lockAbsent,
  ...
}:

# `pullImage`: the image is fetched into the store from what the lockfile records.
let
  lockPull = ./fixtures/pull.lock;
  hash =
    if system == "aarch64-linux" then
      "sha256-AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI="
    else
      "sha256-AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
  digest =
    "sha256:" + lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then "c" else "a"));

  caddy = extra: {
    web = {
      image = "caddy:2-alpine";
    }
    // extra;
  };
  containerOf = c: c.virtualisation.oci-containers.containers.web;
  pulled = eval [
    (with' lockPull (caddy {
      pullImage = true;
    }))
  ];
in
{
  tests = {
    testPullImageIsOffByDefaultAndKeepsTheDigest = {
      expr =
        let
          c = eval [ (with' lockOk (caddy { })) ];
        in
        {
          image = (containerOf c).image;
          file = (containerOf c).imageFile;
          targets = c.virtualisation.pin-oci.lockTargets."docker.io/library/caddy:2-alpine".pullImage;
          failed = failed c;
        };
      expected = {
        image = "docker.io/library/caddy@${digest}";
        file = null;
        targets = false;
        failed = [ ];
      };
    };
    testPullImageBuildsTheStoreImageFromTheLock = {
      expr =
        let
          f = (containerOf pulled).imageFile;
        in
        {
          image = (containerOf pulled).image;
          hash = f.outputHash;
          digest = f.imageDigest;
          source = f.sourceURL;
          dest = f.destNameTag;
          failed = failed pulled;
        };
      expected = {
        image = "docker.io/library/caddy:2-alpine";
        inherit hash digest;
        source = "docker://docker.io/library/caddy@${digest}";
        dest = "docker.io/library/caddy:2-alpine";
        failed = [ ];
      };
    };
    testPullImageOptionIsNotForwardedToOciContainers = {
      expr = (containerOf pulled) ? pullImage;
      expected = false;
    };
    testPullImageIsExposedToTheCliInLockTargets = {
      expr = pulled.virtualisation.pin-oci.lockTargets;
      expected."docker.io/library/caddy:2-alpine" = {
        registry = "docker.io";
        repository = "library/caddy";
        tag = "2-alpine";
        pullImage = true;
      };
    };
    testGlobalPullImageAppliesAndAContainerCanOptOut = {
      expr =
        let
          c = eval [
            {
              virtualisation.pin-oci = {
                enable = true;
                lockFile = lockPull;
                pullImage = true;
                containers = {
                  a.image = "caddy:2-alpine";
                  b = {
                    image = "redis:7";
                    pullImage = false;
                  };
                };
              };
            }
          ];
          cs = c.virtualisation.oci-containers.containers;
        in
        {
          a = cs.a.imageFile != null;
          b = cs.b.imageFile;
          bImage = cs.b.image;
        };
      expected = {
        a = true;
        b = null;
        bImage = "docker.io/library/redis@sha256:${
          lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then "d" else "b"))
        }";
      };
    };
    testOneContainerPullingMakesTheSharedImageATargetToPull = {
      expr =
        (eval [
          (with' lockPull {
            a.image = "caddy:2-alpine";
            b = {
              image = "docker.io/library/caddy:2-alpine";
              pullImage = true;
            };
          })
        ]).virtualisation.pin-oci.lockTargets."docker.io/library/caddy:2-alpine".pullImage;
      expected = true;
    };

    # ---- falsification: the guarantees must hold ----
    testPullImageWithoutNixHashFailsEvaluationWithTheCommand = {
      expr =
        let
          c = eval [
            (with' lockOk (caddy {
              pullImage = true;
            }))
          ];
          msgs = map (a: a.message) (failed c);
        in
        {
          flagged = failedByPinOci c;
          mentionsKey = lib.all (m: lib.hasInfix "docker.io/library/caddy:2-alpine" m) msgs;
          command = lib.all (m: lib.hasInfix "pin-oci lock" m) msgs;
          noContainers = c.virtualisation.oci-containers.containers;
        };
      expected = {
        flagged = [ true ];
        mentionsKey = true;
        command = true;
        noContainers = { };
      };
    };
    testPullImageWithAnAbsentLockfileReportsTheMissingImageNotTheHash = {
      expr = failedByPinOci (eval [
        (with' lockAbsent (caddy {
          pullImage = true;
        }))
      ]);
      expected = [ true ];
    };
    testPullImageOnAPinnedImageIsRejected = {
      expr =
        let
          c = eval [
            (with' lockAbsent {
              web = {
                image = "caddy@sha256:${lib.concatStrings (lib.replicate 64 "b")}";
                pullImage = true;
              };
            })
          ];
        in
        failedByPinOci c;
      expected = [ true ];
    };
    testAnImageNotPulledNeedsNoNixHash = {
      expr = failed (eval [ (with' lockOk (caddy { })) ]);
      expected = [ ];
    };
  };
}
