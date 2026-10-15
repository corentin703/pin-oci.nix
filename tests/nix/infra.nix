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
