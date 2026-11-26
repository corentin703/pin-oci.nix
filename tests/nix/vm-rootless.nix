{ pkgs, module }:

# End-to-end for `podman.user`: real systemd, real rootless podman. The user is declared here, the
# way an integrator declares his own: pin-oci only reads it. No image is pulled; the networks and
# volumes are what a rootless container needs to find in its user's storage.
pkgs.testers.runNixOSTest {
  name = "pin-oci-rootless";

  # A stuck driver must fail with its log long before the CI job is cancelled.
  globalTimeout = 600;

  nodes.machine =
    { ... }:
    {
      imports = [ module ];

      virtualisation.oci-containers.backend = "podman";

      users.groups.oci-app = { };
      users.users.oci-app = {
        isSystemUser = true;
        uid = 2001;
        group = "oci-app";
        home = "/var/lib/oci-app";
        createHome = true;
        linger = true;
        autoSubUidGidRange = false;
        subUidRanges = [
          {
            startUid = 100000;
            count = 65536;
          }
        ];
        subGidRanges = [
          {
            startGid = 100000;
            count = 65536;
          }
        ];
      };

      virtualisation.pin-oci = {
        enable = true;
        lockFile = ./fixtures/ok.lock;

        # Owned by the user, from the host.
        networks.owned = {
          subnet = "10.89.7.0/24";
          podman.user = "oci-app";
        };
        volumes.owned_data.podman.user = "oci-app";

        # Left to root: must stay where it was.
        networks.rooted = { };

        # A project that says once which user manages it.
        projects.app = {
          enable = true;
          podman.user = "oci-app";
          networks.default.enable = true;
          volumes.data = { };
          services.web = {
            image = "caddy:2-alpine";
            autoStart = false;
            volumes = [ "data:/data" ];
          };
        };
      };
    };

  testScript = ''
    import json

    start_all()
    machine.wait_for_unit("multi-user.target")

    def as_user(cmd):
        return f"runuser -u oci-app -- env XDG_RUNTIME_DIR=/run/user/2001 HOME=/var/lib/oci-app {cmd}"

    for unit in ["network-owned", "volume-owned_data", "network-app-default", "volume-app_data", "network-rooted"]:
        machine.wait_for_unit(f"podman-{unit}.service")

    with subtest("owned resources live in the user's storage"):
        machine.succeed(as_user("podman network inspect owned app-default"))
        machine.succeed(as_user("podman volume inspect owned_data app_data"))
        net = json.loads(machine.succeed(as_user("podman network inspect owned")))[0]
        assert net["subnets"][0]["subnet"] == "10.89.7.0/24", net

    with subtest("owned resources are not root's"):
        machine.fail("podman network inspect owned")
        machine.fail("podman volume inspect owned_data")
        machine.fail("podman network inspect app-default")

    with subtest("a resource left to root stays root's"):
        machine.succeed("podman network inspect rooted")
        machine.fail(as_user("podman network inspect rooted"))

    with subtest("the units run as the user, after its manager"):
        assert machine.succeed("systemctl show podman-network-owned.service -p User --value").strip() == "oci-app"
        after = machine.succeed("systemctl show podman-network-owned.service -p After --value").split()
        assert "user@2001.service" in after, after
        assert machine.succeed("systemctl show podman-network-rooted.service -p User --value").strip() == ""

    with subtest("the container unit of the project is ordered after the user's resources"):
        for prop in ["After", "Requires"]:
            value = machine.succeed(f"systemctl show podman-app-web.service -p {prop} --value").split()
            for unit in ["podman-network-app-default.service", "podman-volume-app_data.service"]:
                assert unit in value, (prop, unit, value)

    with subtest("units are idempotent"):
        before = json.loads(machine.succeed(as_user("podman network inspect owned")))[0]["id"]
        machine.succeed("systemctl restart podman-network-owned.service")
        machine.succeed("systemctl restart podman-volume-owned_data.service")
        after = json.loads(machine.succeed(as_user("podman network inspect owned")))[0]["id"]
        assert before == after, "network was recreated by a second run"

    with subtest("a removed network is recreated, still in the user's storage"):
        machine.succeed(as_user("podman network rm owned"))
        machine.fail(as_user("podman network inspect owned"))
        machine.succeed("systemctl restart podman-network-owned.service")
        machine.succeed(as_user("podman network inspect owned"))
        machine.fail("podman network inspect owned")
  '';
}
