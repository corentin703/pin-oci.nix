{
  lib,
  eval,
  evalOn,
  with',
  failed,
  failedByPinOci,
  lockOk,
  ...
}:

# Milestone 2 tests: networks and volumes (unit generation, validation, ordering).
let
  base = with' lockOk { };

  net = {
    virtualisation.pin-oci.networks.app-net = {
      subnet = "10.89.1.0/24";
      internal = true;
    };
  };
  vol = {
    virtualisation.pin-oci.volumes.postgres_data = { };
  };

  infra = eval [
    base
    net
    vol
  ];
  unit = c: name: c.systemd.services.${name};
  netUnit = unit infra "podman-network-app-net";
  volUnit = unit infra "podman-volume-postgres_data";

  has = needle: hay: lib.hasInfix needle hay;

  resourceUnits =
    units: lib.sort lib.lessThan (lib.filter (u: has "-network-" u || has "-volume-" u) units);

  # A locked container (caddy:2-alpine is in ok.lock) with the given extra settings.
  withDb =
    extra:
    eval [
      base
      net
      vol
      { virtualisation.pin-oci.containers.db = { image = "caddy:2-alpine"; } // extra; }
    ];
  orderingOf = c: {
    after = resourceUnits c.systemd.services.podman-db.after;
    requires = resourceUnits c.systemd.services.podman-db.requires;
  };
  both = units: {
    after = units;
    requires = units;
  };
  netAndVol = [
    "podman-network-app-net.service"
    "podman-volume-postgres_data.service"
  ];
in
{
  tests = {
    testNetworkUnitIsAnIdempotentOneshot = {
      expr = {
        inherit (netUnit.serviceConfig) Type RemainAfterExit;
        wantedBy = netUnit.wantedBy;
        checksFirst = has "/bin/podman network inspect app-net >/dev/null 2>&1" netUnit.script;
        thenCreates = has "/bin/podman network create --subnet 10.89.1.0/24 --internal app-net" netUnit.script;
      };
      expected = {
        Type = "oneshot";
        RemainAfterExit = true;
        wantedBy = [ "multi-user.target" ];
        checksFirst = true;
        thenCreates = true;
      };
    };
    testNetworkWithoutOptionsHasNoFlags = {
      expr =
        let
          script =
            (unit (eval [
              base
              { virtualisation.pin-oci.networks.plain = { }; }
            ]) "podman-network-plain"
            ).script;
        in
        {
          creates = has "/bin/podman network create plain" script;
          subnet = has "--subnet" script;
          internal = has "--internal" script;
        };
      expected = {
        creates = true;
        subnet = false;
        internal = false;
      };
    };
    testVolumeUnitUsesTheDriver = {
      expr = {
        inherit (volUnit.serviceConfig) Type RemainAfterExit;
        checksFirst = has "/bin/podman volume inspect postgres_data >/dev/null 2>&1" volUnit.script;
        thenCreates = has "/bin/podman volume create --driver local postgres_data" volUnit.script;
      };
      expected = {
        Type = "oneshot";
        RemainAfterExit = true;
        checksFirst = true;
        thenCreates = true;
      };
    };
    testCustomDriverIsShellEscaped = {
      expr =
        let
          c = eval [
            base
            { virtualisation.pin-oci.volumes.v.driver = "my driver"; }
          ];
        in
        has "volume create --driver 'my driver' v" (unit c "podman-volume-v").script;
      expected = true;
    };
    testDockerBackendUsesDockerAndItsDaemon = {
      expr =
        let
          c = eval [
            base
            net
            { virtualisation.oci-containers.backend = lib.mkForce "docker"; }
          ];
          u = unit c "docker-network-app-net";
        in
        {
          after = lib.elem "docker.service" u.after;
          requires = lib.elem "docker.service" u.requires;
          usesDocker = has "/bin/docker network inspect app-net" u.script;
        };
      expected = {
        after = true;
        requires = true;
        usesDocker = true;
      };
    };
    testPodmanBackendHasNoDockerDaemonDependency = {
      expr = {
        after = lib.elem "docker.service" netUnit.after;
        requires = lib.elem "docker.service" netUnit.requires;
      };
      expected = {
        after = false;
        requires = false;
      };
    };
    testDisabledModuleGeneratesNoResourceUnits = {
      expr =
        let
          c = eval [
            net
            vol
          ];
        in
        lib.filter (n: lib.hasPrefix "podman-network-" n || lib.hasPrefix "podman-volume-" n) (
          builtins.attrNames c.systemd.services
        );
      expected = [ ];
    };
    testNothingDeclaredGeneratesNothing = {
      expr = lib.filter (n: lib.hasPrefix "podman-network-" n || lib.hasPrefix "podman-volume-" n) (
        builtins.attrNames (eval [ base ]).systemd.services
      );
      expected = [ ];
    };
    testValidNamesPass = {
      expr = failed (eval [
        base
        {
          virtualisation.pin-oci.networks."a.b-c_1" = { };
          virtualisation.pin-oci.volumes."0data" = { };
        }
      ]);
      expected = [ ];
    };
    testInvalidNamesFailTheAssertion = {
      expr = lib.genAttrs [ "bad name" "-lead" "a/b" "" "_x" ] (
        name:
        let
          c = eval [
            base
            { virtualisation.pin-oci.networks.${name} = { }; }
          ];
        in
        failedByPinOci c
      );
      expected = lib.genAttrs [ "bad name" "-lead" "a/b" "" "_x" ] (_: [ true ]);
    };
    # ---- container ordering ----
    testContainerIsOrderedAfterItsNetworkAndVolume = {
      expr = orderingOf (withDb {
        volumes = [ "postgres_data:/var/lib/postgresql/data" ];
        extraOptions = [ "--network=app-net" ];
      });
      expected = both netAndVol;
    };
    testEveryNetworkSpellingIsDetected = {
      expr = lib.mapAttrs (_: extra: (orderingOf (withDb extra)).after) {
        networksOption.networks = [ "app-net" ];
        short.extraOptions = [ "--net=app-net" ];
        twoElements.extraOptions = [ "--network" "app-net" ];
        twoElementsShort.extraOptions = [ "--net" "app-net" ];
        withAlias.extraOptions = [ "--network=app-net:alias=db" ];
        optionAlias.networks = [ "app-net:ip=10.89.1.5" ];
        amongOthers.extraOptions = [ "--cap-add=NET_ADMIN" "--network=app-net" "--init" ];
      };
      expected = lib.genAttrs [
        "networksOption"
        "short"
        "twoElements"
        "twoElementsShort"
        "withAlias"
        "optionAlias"
        "amongOthers"
      ] (_: [ "podman-network-app-net.service" ]);
    };
    testVolumeOptionsAreIgnored = {
      expr = (orderingOf (withDb { volumes = [ "postgres_data:/data:ro,Z" ]; })).requires;
      expected = [ "podman-volume-postgres_data.service" ];
    };
    testUndeclaredAndBindMountedResourcesAddNoDependency = {
      expr = orderingOf (withDb {
        networks = [ "host" ];
        extraOptions = [ "--network=bridge" "--network" "unknown-net" ];
        volumes = [
          "/srv:/srv"
          "./rel:/rel"
          "~/home:/home"
          "unknown_vol:/x"
          ":/empty"
        ];
      });
      expected = both [ ];
    };
    testResourcesAreDeduplicated = {
      expr = (orderingOf (withDb {
        networks = [ "app-net" ];
        extraOptions = [ "--network=app-net" ];
        volumes = [ "postgres_data:/a" "postgres_data:/b" ];
      })).requires;
      expected = netAndVol;
    };
    testOrderingKeepsOciContainersOwnDependencies = {
      expr =
        let
          c = eval [
            base
            net
            {
              virtualisation.pin-oci.containers = {
                web.image = "caddy:2-alpine";
                db = {
                  image = "caddy:2-alpine";
                  dependsOn = [ "web" ];
                  networks = [ "app-net" ];
                };
              };
            }
          ];
        in
        {
          requiresWeb = lib.elem "podman-web.service" c.systemd.services.podman-db.requires;
          requiresNet = lib.elem "podman-network-app-net.service" c.systemd.services.podman-db.requires;
          webUntouched = resourceUnits c.systemd.services.podman-web.requires;
        };
      expected = {
        requiresWeb = true;
        requiresNet = true;
        webUntouched = [ ];
      };
    };
    testDockerBackendOrdersAfterDockerResources = {
      expr =
        let
          c = eval [
            base
            net
            vol
            { virtualisation.oci-containers.backend = lib.mkForce "docker"; }
            {
              virtualisation.pin-oci.containers.db = {
                image = "caddy:2-alpine";
                networks = [ "app-net" ];
                volumes = [ "postgres_data:/d" ];
              };
            }
          ];
        in
        resourceUnits c.systemd.services.docker-db.requires;
      expected = [
        "docker-network-app-net.service"
        "docker-volume-postgres_data.service"
      ];
    };
    testUnlockedContainerGetsNoUnitAndNoCrash = {
      expr =
        let
          c = eval [
            (with' ./fixtures/absent.lock { })
            net
            { virtualisation.pin-oci.containers.db = { image = "caddy:2-alpine"; networks = [ "app-net" ]; }; }
          ];
        in
        {
          hasUnit = c.systemd.services ? podman-db;
          failures = failedByPinOci c;
        };
      expected = {
        hasUnit = false;
        failures = [ true ];
      };
    };
    testDisabledModuleLeavesContainersAlone = {
      expr =
        let
          c = eval [
            net
            { virtualisation.pin-oci.containers.db = { image = "caddy:2-alpine"; networks = [ "app-net" ]; }; }
          ];
        in
        c.systemd.services ? podman-db;
      expected = false;
    };
    testInvalidVolumeNameIsNamedInTheMessage = {
      expr =
        let
          c = eval [
            base
            { virtualisation.pin-oci.volumes."bad vol" = { }; }
          ];
        in
        has "bad vol" (builtins.head (failed c)).message;
      expected = true;
    };
  };
}
