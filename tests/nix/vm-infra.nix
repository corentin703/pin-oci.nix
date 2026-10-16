{ pkgs, module }:

# End-to-end: real systemd, real podman and docker. No image is pulled: only the network and
# volume units run, container ordering is read back from systemd.
pkgs.testers.runNixOSTest {
  name = "pin-oci-infra";

  # A stuck driver must fail with its log long before the CI job is cancelled.
  globalTimeout = 600;

  defaults =
    { lib, ... }:
    {
      imports = [ module ];
      virtualisation.pin-oci = {
        enable = true;
        lockFile = ./fixtures/ok.lock;
        networks.app-net = {
          subnet = "10.89.1.0/24";
          internal = true;
        };
        networks.plain = { };
        volumes.pgdata = { };
        containers.db = {
          image = "caddy:2-alpine";
          autoStart = false;
          networks = [ "app-net" ];
          volumes = [ "pgdata:/data" ];
        };
      };
    };

  nodes.podman =
    { ... }:
    {
      virtualisation.oci-containers.backend = "podman";
    };

  nodes.docker =
    { ... }:
    {
      virtualisation.oci-containers.backend = "docker";
    };

  testScript = ''
    import json

    start_all()

    def inspect(machine, backend, kind, name):
        return json.loads(machine.succeed(f"{backend} {kind} inspect {name}"))[0]

    for machine, backend in [(podman, "podman"), (docker, "docker")]:
        with subtest(f"{backend}: resources are created at boot"):
            machine.wait_for_unit("multi-user.target")
            for unit in ["network-app-net", "network-plain", "volume-pgdata"]:
                machine.wait_for_unit(f"{backend}-{unit}.service")
            machine.succeed(f"{backend} network inspect app-net plain >/dev/null")
            machine.succeed(f"{backend} volume inspect pgdata >/dev/null")

        with subtest(f"{backend}: declared flags are applied"):
            net = inspect(machine, backend, "network", "app-net")
            assert net["Internal" if backend == "docker" else "internal"] is True, net
            subnet = (
                net["IPAM"]["Config"][0]["Subnet"]
                if backend == "docker"
                else net["subnets"][0]["subnet"]
            )
            assert subnet == "10.89.1.0/24", subnet
            plain = inspect(machine, backend, "network", "plain")
            assert plain["Internal" if backend == "docker" else "internal"] is False, plain

        with subtest(f"{backend}: units are idempotent"):
            key = "Id" if backend == "docker" else "id"
            before = inspect(machine, backend, "network", "app-net")[key]
            machine.succeed(f"systemctl restart {backend}-network-app-net.service")
            machine.succeed(f"systemctl restart {backend}-volume-pgdata.service")
            after = inspect(machine, backend, "network", "app-net")[key]
            assert before == after, "network was recreated by a second run"

        with subtest(f"{backend}: a removed network is recreated"):
            machine.succeed(f"{backend} network rm app-net")
            machine.fail(f"{backend} network inspect app-net")
            machine.succeed(f"systemctl restart {backend}-network-app-net.service")
            machine.succeed(f"{backend} network inspect app-net")

        with subtest(f"{backend}: the container unit is ordered after its resources"):
            for prop in ["After", "Requires"]:
                value = machine.succeed(f"systemctl show {backend}-db.service -p {prop} --value")
                for unit in [f"{backend}-network-app-net.service", f"{backend}-volume-pgdata.service"]:
                    assert unit in value.split(), (prop, unit, value)
            unrelated = machine.succeed(f"systemctl show {backend}-db.service -p Requires --value")
            assert f"{backend}-network-plain.service" not in unrelated.split(), unrelated
  '';
}
