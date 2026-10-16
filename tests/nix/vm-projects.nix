{ pkgs, module }:

# End-to-end for projects, with real podman and docker. The pinned images cannot run offline, so
# the generated `networks` entries are replayed on a locally built probe image: this checks
# that the backend accepts them and that a service resolves by its bare name.
let
  probe = pkgs.dockerTools.buildImage {
    name = "probe";
    tag = "latest";
    copyToRoot = pkgs.buildEnv {
      name = "probe-root";
      paths = [ pkgs.busybox ];
      pathsToLink = [ "/bin" ];
    };
    config.Cmd = [
      "sleep"
      "600"
    ];
  };
in
pkgs.testers.runNixOSTest {
  name = "pin-oci-projects";

  defaults = {
    imports = [ module ];
    virtualisation.pin-oci = {
      enable = true;
      lockFile = ./fixtures/ok.lock;
      networks.shared = { };
      volumes.shared_data = { };
      projects.monitoring = {
        enable = true;
        networks.default.enable = true;
        networks.backend.internal = true;
        networks.shared.external = true;
        volumes.prom_data = { };
        volumes.shared_data.external = true;
        services = {
          prometheus = {
            image = "caddy:2-alpine";
            autoStart = false;
            volumes = [
              "prom_data:/prometheus"
              "shared_data:/shared"
            ];
            networks = [ "backend" ];
          };
          grafana = {
            image = "redis:7";
            autoStart = false;
            dependsOn = [ "prometheus" ];
            networks = [ "shared" ];
          };
        };
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

  testScript =
    { nodes, ... }:
    let
      entries =
        backend: container: nodes.${backend}.virtualisation.oci-containers.containers.${container}.networks;
    in
    ''
      import json
      import shlex

      start_all()

      PROMETHEUS_NETWORKS = {
          "podman": ${builtins.toJSON (entries "podman" "monitoring-prometheus")},
          "docker": ${builtins.toJSON (entries "docker" "monitoring-prometheus")},
      }

      for machine, backend in [(podman, "podman"), (docker, "docker")]:
          with subtest(f"{backend}: project resources are created with prefixed names"):
              machine.wait_for_unit("multi-user.target")
              for unit in [
                  "network-monitoring-default",
                  "network-monitoring-backend",
                  "volume-monitoring_prom_data",
              ]:
                  machine.wait_for_unit(f"{backend}-{unit}.service")
              machine.succeed(f"{backend} network inspect monitoring-default monitoring-backend >/dev/null")
              machine.succeed(f"{backend} volume inspect monitoring_prom_data >/dev/null")
              machine.fail(f"{backend} network inspect default")
              machine.fail(f"{backend} volume inspect prom_data")

          with subtest(f"{backend}: external resources are the global ones"):
              machine.wait_for_unit(f"{backend}-network-shared.service")
              machine.wait_for_unit(f"{backend}-volume-shared_data.service")
              machine.fail(f"{backend} network inspect monitoring-shared")
              machine.fail(f"{backend} volume inspect monitoring_shared_data")

          with subtest(f"{backend}: the backend network is internal"):
              net = json.loads(machine.succeed(f"{backend} network inspect monitoring-backend"))[0]
              assert net["Internal" if backend == "docker" else "internal"] is True, net

          with subtest(f"{backend}: systemd orders services after dependencies and resources"):
              def requires(unit):
                  return machine.succeed(f"systemctl show {unit} -p Requires --value").split()
              def after(unit):
                  return machine.succeed(f"systemctl show {unit} -p After --value").split()

              prometheus = f"{backend}-monitoring-prometheus.service"
              grafana = f"{backend}-monitoring-grafana.service"
              for unit in [
                  f"{backend}-network-monitoring-default.service",
                  f"{backend}-network-monitoring-backend.service",
                  f"{backend}-volume-monitoring_prom_data.service",
                  f"{backend}-volume-shared_data.service",
              ]:
                  assert unit in requires(prometheus), (unit, requires(prometheus))
                  assert unit in after(prometheus), (unit, after(prometheus))
              assert prometheus in requires(grafana), requires(grafana)
              assert prometheus in after(grafana), after(grafana)
              assert f"{backend}-network-shared.service" in requires(grafana), requires(grafana)

          with subtest(f"{backend}: a service resolves by its bare name on the default network"):
              machine.succeed(f"{backend} load -i ${probe}")
              flags = " ".join(f"--network={shlex.quote(n)}" for n in PROMETHEUS_NETWORKS[backend])
              machine.succeed(
                  f"{backend} run -d --name monitoring-prometheus {flags} probe:latest"
              )
              machine.succeed(
                  f"{backend} run --rm --network monitoring-default probe:latest nslookup prometheus"
              )
              machine.succeed(
                  f"{backend} run --rm --network monitoring-default probe:latest nslookup monitoring-prometheus"
              )
              machine.fail(
                  f"{backend} run --rm --network monitoring-default probe:latest nslookup grafana"
              )
    '';
}
