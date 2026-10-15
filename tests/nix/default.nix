{
  pkgs,
  nixpkgs,
  module,
}:

# Offline module tests: unit (parseRef), integration (module evaluation against fixture
# lockfiles) and e2e-equivalent (generated systemd units). Evaluated purely, no network.
let
  helpers = import ./common.nix { inherit pkgs nixpkgs module; };
  inherit (helpers)
    lib
    system
    evalOn
    eval
    with'
    failed
    failedByPinOci
    fails
    web
    lockOk
    lockWrongArch
    lockNoArch
    lockAbsent
    ;
  inherit (import ../../modules/refs.nix { inherit lib; }) parseRef;

  apply = import ../../src/apply.nix;

  caddyDigest = "sha256:" + lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then "c" else "a"));
  caddyPinned = "docker.io/library/caddy@${caddyDigest}";

  locked = eval [ (with' lockOk web) ];
  missing = eval [ (with' lockAbsent web) ];

  refCase = r: expected: {
    expr = parseRef r;
    inherit expected;
  };
  canonical = key: registry: repository: tag: pinned: {
    inherit
      key
      registry
      repository
      tag
      pinned
      ;
  };

  tests = {
    # ---- unit: reference normalization ----
    testRefLibrary = refCase "redis:7" (canonical "docker.io/library/redis:7" "docker.io" "library/redis" "7" false);
    testRefNoTagIsLatest = refCase "redis" (canonical "docker.io/library/redis:latest" "docker.io" "library/redis" "latest" false);
    testRefUserRepo = refCase "prom/prometheus:v2.50.0" (canonical "docker.io/prom/prometheus:v2.50.0" "docker.io" "prom/prometheus" "v2.50.0" false);
    testRefExplicitDockerIo = refCase "docker.io/redis:7" (canonical "docker.io/library/redis:7" "docker.io" "library/redis" "7" false);
    testRefFullyQualified = refCase "docker.io/library/redis:7" (canonical "docker.io/library/redis:7" "docker.io" "library/redis" "7" false);
    testRefOtherRegistry = refCase "ghcr.io/a/b:1" (canonical "ghcr.io/a/b:1" "ghcr.io" "a/b" "1" false);
    testRefRegistryPort = refCase "localhost:5000/foo:1" (canonical "localhost:5000/foo:1" "localhost:5000" "foo" "1" false);
    testRefRegistryPortNoTag = refCase "registry:5000/x/y" (canonical "registry:5000/x/y:latest" "registry:5000" "x/y" "latest" false);
    testRefLocalhost = refCase "localhost/foo" (canonical "localhost/foo:latest" "localhost" "foo" "latest" false);
    testRefDigestIsPinned = {
      expr = (parseRef "redis@sha256:${lib.concatStrings (lib.replicate 64 "b")}").pinned;
      expected = true;
    };
    testRefTagAndDigestIsPinned = {
      expr = (parseRef "ghcr.io/a/b:1@sha256:${lib.concatStrings (lib.replicate 64 "b")}").pinned;
      expected = true;
    };
    testRefSpellingsShareAKey = {
      expr = lib.unique (map (r: (parseRef r).key) [ "redis:7" "docker.io/redis:7" "docker.io/library/redis:7" ]);
      expected = [ "docker.io/library/redis:7" ];
    };
    testRefEmptyTagRejected = {
      expr = fails (parseRef "redis:");
      expected = true;
    };
    testRefEmptyRepositoryRejected = {
      expr = fails (parseRef "ghcr.io/");
      expected = true;
    };

    # ---- integration: locked configuration ----
    testLockedHasNoFailedAssertion = {
      expr = failed locked;
      expected = [ ];
    };
    testLockedImageIsPinnedToTheHostDigest = {
      expr = locked.virtualisation.oci-containers.containers.web.image;
      expected = caddyPinned;
    };
    testLockedOptionsAreCarriedOver = {
      expr =
        let
          c = locked.virtualisation.oci-containers.containers.web;
        in
        {
          inherit (c) ports volumes environment;
        };
      expected = {
        ports = [ "80:80" ];
        volumes = [ "/srv:/srv:ro" ];
        environment.FOO = "bar";
      };
    };
    testLockTargetsOfLockedConfig = {
      expr = locked.virtualisation.pin-oci.lockTargets;
      expected."docker.io/library/caddy:2-alpine" = {
        registry = "docker.io";
        repository = "library/caddy";
        tag = "2-alpine";
      };
    };

    testDigestFollowsTheHostPlatform = {
      expr = lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (
        sys: (evalOn sys [ (with' lockOk web) ]).virtualisation.oci-containers.containers.web.image
      );
      expected = {
        x86_64-linux = "docker.io/library/caddy@sha256:${lib.concatStrings (lib.replicate 64 "a")}";
        aarch64-linux = "docker.io/library/caddy@sha256:${lib.concatStrings (lib.replicate 64 "c")}";
      };
    };
    testCliContractReportsTheModulePlatform = {
      expr = lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (sys: (apply (evalOn sys [ (with' lockAbsent web) ])).system);
      expected = {
        x86_64-linux = "x86_64-linux";
        aarch64-linux = "aarch64-linux";
      };
    };

    # ---- e2e-equivalent: the generated systemd unit runs the digest, not the tag ----
    testUnitRunsThePinnedImage = {
      expr =
        let
          script = locked.systemd.services."podman-web".script;
        in
        {
          pinned = lib.hasInfix caddyPinned script;
          floatingTag = lib.hasInfix "caddy:2-alpine" script;
        };
      expected = {
        pinned = true;
        floatingTag = false;
      };
    };
    testDockerBackendUnit = {
      expr =
        let
          c = eval [
            (with' lockOk web)
            { virtualisation.oci-containers.backend = lib.mkForce "docker"; }
          ];
        in
        lib.hasInfix caddyPinned c.systemd.services."docker-web".script;
      expected = true;
    };

    # ---- fail-fast: unlocked images ----
    testMissingLockfileFailsTheAssertion = {
      expr = failedByPinOci missing;
      expected = [ true ];
    };
    testMissingLockfileMessageGivesTheCommand = {
      expr =
        let
          m = (builtins.head (failed missing)).message;
        in
        map (s: lib.hasInfix s m) [
          "pin-oci lock"
          "git add pin-oci.lock"
          "docker.io/library/caddy:2-alpine"
          system
        ];
      expected = [ true true true true ];
    };
    testMissingLockfileLowersNothing = {
      expr = missing.virtualisation.oci-containers.containers;
      expected = { };
    };
    testWrongArchitectureFails = {
      expr = failedByPinOci (eval [ (with' lockWrongArch web) ]);
      expected = [ true ];
    };
    testEntryWithoutArchIsMissingNotACrash = {
      expr = failedByPinOci (eval [ (with' lockNoArch web) ]);
      expected = [ true ];
    };
    testOneMissingImageAmongLockedOnesFails = {
      expr =
        let
          c = eval [ (with' lockOk (web // { db.image = "postgres:16-alpine"; })) ];
          m = (builtins.head (failed c)).message;
        in
        {
          failing = builtins.length (failed c);
          names = lib.hasInfix "postgres:16-alpine" m;
          doesNotNameLocked = !(lib.hasInfix "caddy" m);
        };
      expected = {
        failing = 1;
        names = true;
        doesNotNameLocked = true;
      };
    };

    # ---- laziness guarantee: the CLI can evaluate without any lockfile ----
    testLockTargetsEvaluateWithoutLockfile = {
      expr = builtins.attrNames missing.virtualisation.pin-oci.lockTargets;
      expected = [ "docker.io/library/caddy:2-alpine" ];
    };
    testLockTargetsEvaluateWithoutLockFileOption = {
      expr = builtins.attrNames (eval [ { virtualisation.pin-oci.containers = web; } ]).virtualisation.pin-oci.lockTargets;
      expected = [ "docker.io/library/caddy:2-alpine" ];
    };
    testCliContractShape = {
      expr = apply missing;
      expected = {
        inherit system;
        targets."docker.io/library/caddy:2-alpine" = {
          registry = "docker.io";
          repository = "library/caddy";
          tag = "2-alpine";
        };
      };
    };

    # ---- pass-through and edge cases ----
    testPinnedImageIsNotATargetAndStaysUntouched = {
      expr =
        let
          img = "redis@sha256:${lib.concatStrings (lib.replicate 64 "b")}";
          c = eval [ (with' lockAbsent { cache.image = img; }) ];
        in
        {
          targets = c.virtualisation.pin-oci.lockTargets;
          failed = failed c;
          image = c.virtualisation.oci-containers.containers.cache.image;
          expected = img;
        };
      expected = {
        targets = { };
        failed = [ ];
        image = "redis@sha256:${lib.concatStrings (lib.replicate 64 "b")}";
        expected = "redis@sha256:${lib.concatStrings (lib.replicate 64 "b")}";
      };
    };
    testDifferentSpellingsOfOneImageShareOneTarget = {
      expr =
        let
          c = eval [
            (with' lockOk {
              a.image = "redis:7";
              b.image = "docker.io/library/redis:7";
            })
          ];
        in
        {
          targets = builtins.attrNames c.virtualisation.pin-oci.lockTargets;
          failed = failed c;
          images = lib.unique (lib.mapAttrsToList (_: v: v.image) c.virtualisation.oci-containers.containers);
        };
      expected = {
        targets = [ "docker.io/library/redis:7" ];
        failed = [ ];
        images = [ "docker.io/library/redis@sha256:${lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then "d" else "b"))}" ];
      };
    };
    testDisabledModuleAddsNothing = {
      expr =
        let
          c = eval [ { virtualisation.pin-oci.containers = web; } ];
        in
        {
          failed = failed c;
          containers = c.virtualisation.oci-containers.containers;
        };
      expected = {
        failed = [ ];
        containers = { };
      };
    };
    testLockFileIsMandatoryWhenEnabled = {
      expr = fails (map (a: a.assertion) (eval [ { virtualisation.pin-oci = { enable = true; containers = web; }; } ]).assertions);
      expected = true;
    };
    testInvalidReferenceFailsEvaluation = {
      expr = fails (eval [ (with' lockOk { bad.image = "redis:"; }) ]).virtualisation.pin-oci.lockTargets;
      expected = true;
    };
  };

  allTests = tests // (import ./infra.nix helpers).tests;
  failures = lib.debug.runTests allTests;
in
pkgs.runCommand "pin-oci-module-tests" { } (
  if failures == [ ] then
    ''
      echo "${toString (builtins.length (builtins.attrNames allTests))} module tests passed"
      touch $out
    ''
  else
    ''
      echo ${lib.escapeShellArg (builtins.toJSON failures)} >&2
      exit 1
    ''
)
