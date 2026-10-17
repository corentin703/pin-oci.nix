{
  lib,
  eval,
  with',
  failed,
  lockOk,
  ...
}:

# `pin-oci convert` output, evaluated by the real module. The reference modules in
# tests/fixtures/compose/ are the very files tests/convert_cli.rs compares the binary's output
# to, so a conversion the module would reject cannot stay green on either side.
let
  golden = name: import (../fixtures/compose + "/${name}.nix");
  base = with' lockOk { };
  # shop.yml marks `shared` external: it must exist as a global volume.
  sharedVolume = {
    virtualisation.pin-oci.volumes.shared = { };
  };

  monitoring = eval [
    base
    (golden "monitoring")
  ];
  shop = eval [
    base
    sharedVolume
    (golden "shop")
  ];
  withoutExternal = eval [
    base
    (golden "shop")
  ];

  containers = c: c.virtualisation.oci-containers.containers;
  unit = c: name: c.systemd.services.${name};
  web = (containers shop).shop-web;
in
{
  tests = {
    # ---- monitoring.yml: the README project ----
    testMonitoringIsAcceptedByTheModule = {
      expr = failed monitoring;
      expected = [ ];
    };
    testMonitoringServicesBecomePrefixedContainers = {
      expr = builtins.attrNames (containers monitoring);
      expected = [
        "monitoring-grafana"
        "monitoring-prometheus"
      ];
    };
    testMonitoringDependsOnBecomesUnitOrdering = {
      expr = (unit monitoring "podman-monitoring-grafana").requires;
      expected = [
        "podman-monitoring-prometheus.service"
        "podman-network-monitoring-default.service"
        "podman-volume-monitoring_grafana_data.service"
      ];
    };
    testMonitoringGetsItsDefaultNetworkAndVolumes = {
      expr = {
        networks = builtins.attrNames monitoring.virtualisation.pin-oci.networks;
        volumes = builtins.attrNames monitoring.virtualisation.pin-oci.volumes;
      };
      expected = {
        networks = [ "monitoring-default" ];
        volumes = [
          "monitoring_grafana_data"
          "monitoring_prom_data"
        ];
      };
    };

    # ---- shop.yml: anchors, long syntaxes, networks, an external volume ----
    testShopIsAcceptedByTheModule = {
      expr = failed shop;
      expected = [ ];
    };
    testShopContainers = {
      expr = builtins.attrNames (containers shop);
      expected = [
        "shop-cache"
        "shop-web"
      ];
    };
    testShopServiceOptionsAreCarriedOver = {
      expr = {
        inherit (web)
          entrypoint
          cmd
          environment
          labels
          ports
          user
          workdir
          ;
      };
      expected = {
        entrypoint = "/usr/bin/caddy";
        cmd = [
          "run"
          "caddy"
          "file-server"
          "--root"
          "/srv/site docs"
        ];
        # `DEBUG: no` is the text "no", not a YAML 1.1 boolean.
        environment = {
          DEBUG = "no";
          GREETING = "hello world";
          MODE = "prod";
        };
        labels.team = "platform";
        ports = [
          "80:80"
          "127.0.0.1:8443:443"
        ];
        user = "1000:1000";
        workdir = "/srv";
      };
    };
    testShopVolumesArePrefixedUnlessBindOrExternal = {
      expr = web.volumes;
      expected = [
        "shop_site:/srv"
        "/etc/caddy:/etc/caddy:ro"
        "shared:/shared"
      ];
    };
    testShopNetworksKeepTheirSettingsAndThereIsNoDefault = {
      expr = {
        names = builtins.attrNames shop.virtualisation.pin-oci.networks;
        back = {
          inherit (shop.virtualisation.pin-oci.networks.shop-back) internal subnet;
        };
      };
      expected = {
        names = [
          "shop-back"
          "shop-front"
        ];
        back = {
          internal = true;
          subnet = "10.30.0.0/24";
        };
      };
    };
    testShopExternalVolumeIsNotRecreatedByTheProject = {
      expr = builtins.attrNames shop.virtualisation.pin-oci.volumes;
      expected = [
        "shared"
        "shop_site"
      ];
    };

    # Falsification of the method: without the global volume the module rejects shop.nix, so
    # a generated file the module would refuse cannot pass these tests.
    testTheModuleRejectsAnExternalVolumeThatDoesNotExist = {
      expr = lib.any (a: lib.hasInfix "shared" a.message) (failed withoutExternal);
      expected = true;
    };
  };
}
