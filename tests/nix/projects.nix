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

# Milestone 3 tests: projects compiled to containers, networks and volumes.
let
  base = with' lockOk { };
  digest = c: "docker.io/library/caddy@sha256:${lib.concatStrings (lib.replicate 64 c)}";

  # The BIBLE example, with images present in ok.lock.
  monitoring = {
    virtualisation.pin-oci.projects.monitoring = {
      enable = true;
      networks.default.enable = true;
      services = {
        prometheus = {
          image = "caddy:2-alpine";
          volumes = [ "prom_data:/prometheus" ];
          ports = [ "9090:9090" ];
        };
        grafana = {
          image = "redis:7";
          dependsOn = [ "prometheus" ];
          ports = [ "3000:3000" ];
          volumes = [ "grafana_data:/var/lib/grafana" ];
        };
      };
      volumes.prom_data = { };
      volumes.grafana_data = { };
    };
  };

  mon = eval [
    base
    monitoring
  ];
  containers = c: c.virtualisation.oci-containers.containers;
  svc = c: name: (containers c).${name};
  project = extra: {
    virtualisation.pin-oci.projects.p = {
      enable = true;
    }
    // extra;
  };
  evalProject =
    extra:
    eval [
      base
      (project extra)
    ];
  has = needle: hay: lib.hasInfix needle hay;
  ordered = c: unit: c.systemd.services.${unit}.requires;
in
{
  tests = {
    # ---- naming and lowering ----
    testServicesBecomePrefixedContainers = {
      expr = builtins.attrNames (containers mon);
      expected = [
        "monitoring-grafana"
        "monitoring-prometheus"
      ];
    };
    testNoFailedAssertion = {
      expr = failed mon;
      expected = [ ];
    };
    testServiceOptionsAreCarriedOver = {
      expr = {
        inherit (svc mon "monitoring-prometheus") ports;
        image = (svc mon "monitoring-prometheus").image;
      };
      expected = {
        ports = [ "9090:9090" ];
        image = digest (if system == "aarch64-linux" then "c" else "a");
      };
    };
    # Regression: serviceName used to keep the unprefixed default (podman-grafana).
    testServiceUnitFollowsThePrefixedName = {
      expr = lib.genAttrs [
        "podman-monitoring-grafana"
        "podman-monitoring-prometheus"
        "podman-grafana"
        "podman-prometheus"
      ] (n: mon.systemd.services ? ${n});
      expected = {
        podman-monitoring-grafana = true;
        podman-monitoring-prometheus = true;
        podman-grafana = false;
        podman-prometheus = false;
      };
    };
    # ---- project target ----
    testProjectTargetGroupsTheServices = {
      expr = {
        wants = lib.sort lib.lessThan mon.systemd.targets.podman-monitoring.wants;
        boot = mon.systemd.targets.podman-monitoring.wantedBy;
      };
      expected = {
        wants = [
          "podman-monitoring-grafana.service"
          "podman-monitoring-prometheus.service"
        ];
        boot = [ "multi-user.target" ];
      };
    };
    testServicesArePartOfTheProjectTarget = {
      expr = map (n: mon.systemd.services.${n}.partOf) [
        "podman-monitoring-grafana"
        "podman-monitoring-prometheus"
      ];
      expected = [
        [ "podman-monitoring.target" ]
        [ "podman-monitoring.target" ]
      ];
    };
    testTargetFollowsAnExplicitServiceName = {
      expr =
        let
          c = evalProject {
            services.web = {
              image = "caddy:2-alpine";
              serviceName = "custom-web";
            };
          };
        in
        {
          wants = c.systemd.targets.podman-p.wants;
          partOf = c.systemd.services.custom-web.partOf;
        };
      expected = {
        wants = [ "custom-web.service" ];
        partOf = [ "podman-p.target" ];
      };
    };
    testDisabledProjectHasNoTarget = {
      expr =
        (eval [
          base
          { virtualisation.pin-oci.projects.p.services.web.image = "caddy:2-alpine"; }
        ]).systemd.targets
          ? podman-p;
      expected = false;
    };
    testNoTargetWithoutProject = {
      expr = lib.filter (n: lib.hasPrefix "podman-" n) (
        builtins.attrNames (eval [ base ]).systemd.targets
      );
      expected = [ ];
    };
    testExplicitServiceNameIsKept = {
      expr =
        (evalProject {
          services.web = {
            image = "caddy:2-alpine";
            serviceName = "custom-web";
          };
        }).virtualisation.oci-containers.containers.p-web.serviceName;
      expected = "custom-web";
    };
    testGlobalAndProjectContainersCoexist = {
      expr = builtins.attrNames (
        containers (eval [
          (with' lockOk { plain.image = "redis:7"; })
          (project { services.web.image = "caddy:2-alpine"; })
        ])
      );
      expected = [
        "p-web"
        "plain"
      ];
    };
    testDisabledProjectGeneratesNothing = {
      expr =
        let
          c = eval [
            base
            {
              virtualisation.pin-oci.projects.p = {
                services.web.image = "caddy:2-alpine";
                volumes.v = { };
                networks.n = { };
              };
            }
          ];
        in
        {
          containers = builtins.attrNames (containers c);
          networks = builtins.attrNames c.virtualisation.pin-oci.networks;
          volumes = builtins.attrNames c.virtualisation.pin-oci.volumes;
        };
      expected = {
        containers = [ ];
        networks = [ ];
        volumes = [ ];
      };
    };
    testModuleDisabledGeneratesNothing = {
      expr =
        let
          c = eval [
            {
              virtualisation.pin-oci.projects.p = {
                enable = true;
                services.web.image = "caddy:2-alpine";
              };
            }
          ];
        in
        builtins.attrNames (containers c);
      expected = [ ];
    };

    # ---- the default network and DNS ----
    testDefaultNetworkIsCreatedAndPrefixed = {
      expr = builtins.attrNames mon.virtualisation.pin-oci.networks;
      expected = [ "monitoring-default" ];
    };
    testServicesJoinTheDefaultNetworkWithTheirNameAsAlias = {
      expr = {
        prometheus = (svc mon "monitoring-prometheus").networks;
        grafana = (svc mon "monitoring-grafana").networks;
      };
      expected = {
        prometheus = [ "monitoring-default:alias=prometheus" ];
        grafana = [ "monitoring-default:alias=grafana" ];
      };
    };
    testDockerUsesTheCsvAliasSyntax = {
      expr =
        (eval [
          base
          monitoring
          { virtualisation.oci-containers.backend = lib.mkForce "docker"; }
        ]).virtualisation.oci-containers.containers.monitoring-prometheus.networks;
      expected = [ "name=monitoring-default,alias=prometheus" ];
    };
    testNoDefaultNetworkUnlessEnabled = {
      expr =
        let
          c = evalProject { services.web.image = "caddy:2-alpine"; };
        in
        {
          networks = builtins.attrNames c.virtualisation.pin-oci.networks;
          joined = (svc c "p-web").networks;
        };
      expected = {
        networks = [ ];
        joined = [ ];
      };
    };
    testDeclaredDefaultNetworkStaysOffUntilEnabled = {
      expr = builtins.attrNames (evalProject { networks.default = { }; }).virtualisation.pin-oci.networks;
      expected = [ ];
    };
    testDefaultNetworkCanBeConfigured = {
      expr =
        (evalProject {
          networks.default = {
            enable = true;
            subnet = "10.5.0.0/24";
            internal = true;
          };
        }).virtualisation.pin-oci.networks.p-default;
      expected = {
        subnet = "10.5.0.0/24";
        internal = true;
      };
    };
    testDefaultNetworkCannotBeExternal = {
      expr = failedByPinOci (evalProject {
        networks.default = {
          enable = true;
          external = true;
        };
      });
      expected = [ true ];
    };

    # ---- volumes ----
    testProjectVolumesArePrefixedAndDeclaredGlobally = {
      expr = mon.virtualisation.pin-oci.volumes;
      expected = {
        monitoring_prom_data.driver = "local";
        monitoring_grafana_data.driver = "local";
      };
    };
    testVolumeReferencesAreRewritten = {
      expr = {
        prometheus = (svc mon "monitoring-prometheus").volumes;
        grafana = (svc mon "monitoring-grafana").volumes;
      };
      expected = {
        prometheus = [ "monitoring_prom_data:/prometheus" ];
        grafana = [ "monitoring_grafana_data:/var/lib/grafana" ];
      };
    };
    testVolumeOptionsSurviveTheRewrite = {
      expr =
        (svc (evalProject {
          services.web = {
            image = "caddy:2-alpine";
            volumes = [ "v:/d:ro,Z" ];
          };
          volumes.v = { };
        }) "p-web").volumes;
      expected = [ "p_v:/d:ro,Z" ];
    };
    testBindMountsAndUndeclaredVolumesAreUntouched = {
      expr =
        (svc (evalProject {
          services.web = {
            image = "caddy:2-alpine";
            volumes = [
              "/srv:/srv"
              "./rel:/r"
              "mine:/m"
            ];
          };
          volumes.v = { };
        }) "p-web").volumes;
      expected = [
        "/srv:/srv"
        "./rel:/r"
        "mine:/m"
      ];
    };
    testVolumeDriverIsForwarded = {
      expr = (evalProject { volumes.v.driver = "nfs"; }).virtualisation.pin-oci.volumes.p_v.driver;
      expected = "nfs";
    };
    testSameVolumeNameInTwoProjectsDoesNotCollide = {
      expr =
        builtins.attrNames
          (eval [
            base
            {
              virtualisation.pin-oci.projects = {
                a = {
                  enable = true;
                  volumes.data = { };
                };
                b = {
                  enable = true;
                  volumes.data = { };
                };
              };
            }
          ]).virtualisation.pin-oci.volumes;
      expected = [
        "a_data"
        "b_data"
      ];
    };

    # ---- external resources ----
    testExternalVolumeIsNotPrefixedNorGenerated = {
      expr =
        let
          c = eval [
            (with' lockOk { })
            { virtualisation.pin-oci.volumes.shared = { }; }
            (project {
              services.web = {
                image = "caddy:2-alpine";
                volumes = [ "shared:/s" ];
              };
              volumes.shared.external = true;
            })
          ];
        in
        {
          volumes = builtins.attrNames c.virtualisation.pin-oci.volumes;
          reference = (svc c "p-web").volumes;
          failed = failed c;
          ordered = lib.elem "podman-volume-shared.service" (ordered c "podman-p-web");
        };
      expected = {
        volumes = [ "shared" ];
        reference = [ "shared:/s" ];
        failed = [ ];
        ordered = true;
      };
    };
    testExternalNetworkIsNotPrefixedNorGenerated = {
      expr =
        let
          c = eval [
            (with' lockOk { })
            { virtualisation.pin-oci.networks.shared = { }; }
            (project {
              services.web = {
                image = "caddy:2-alpine";
                networks = [ "shared:alias=w" ];
              };
              networks.shared.external = true;
            })
          ];
        in
        {
          networks = builtins.attrNames c.virtualisation.pin-oci.networks;
          reference = (svc c "p-web").networks;
          failed = failed c;
        };
      expected = {
        networks = [ "shared" ];
        reference = [ "shared:alias=w" ];
        failed = [ ];
      };
    };
    testMissingExternalResourcesFailTheAssertion = {
      expr =
        let
          c = evalProject {
            volumes.nope.external = true;
            networks.nowhere.external = true;
          };
          m = (builtins.head (failed c)).message;
        in
        {
          failures = failedByPinOci c;
          names = [
            (has "volume 'nope'" m)
            (has "network 'nowhere'" m)
          ];
        };
      expected = {
        failures = [ true ];
        names = [
          true
          true
        ];
      };
    };

    # ---- network references ----
    testProjectNetworkReferencesAreRewritten = {
      expr =
        let
          c = evalProject {
            networks.backend = {
              subnet = "10.6.0.0/24";
              internal = true;
            };
            services.db = {
              image = "caddy:2-alpine";
              networks = [ "backend:alias=db" ];
              extraOptions = [
                "--network=backend"
                "--net"
                "backend"
                "--network=host"
              ];
            };
          };
        in
        {
          created = c.virtualisation.pin-oci.networks;
          networks = (svc c "p-db").networks;
          extraOptions = (svc c "p-db").extraOptions;
        };
      expected = {
        created.p-backend = {
          subnet = "10.6.0.0/24";
          internal = true;
        };
        networks = [ "p-backend:alias=db" ];
        extraOptions = [
          "--network=p-backend"
          "--net"
          "p-backend"
          "--network=host"
        ];
      };
    };
    testDockerCsvNetworkReferenceIsRewritten = {
      expr =
        (svc (eval [
          base
          (project {
            networks.backend = { };
            services.db = {
              image = "caddy:2-alpine";
              networks = [ "name=backend,alias=db" ];
            };
          })
          { virtualisation.oci-containers.backend = lib.mkForce "docker"; }
        ]) "p-db").networks;
      expected = [ "name=p-backend,alias=db" ];
    };
    testServiceNamedLikeANetworkIsNotRewritten = {
      expr =
        (svc (evalProject {
          networks.backend = { };
          services.backend = {
            image = "caddy:2-alpine";
            extraOptions = [ "--hostname=backend" ];
          };
        }) "p-backend").extraOptions;
      expected = [ "--hostname=backend" ];
    };

    # ---- dependsOn ----
    testDependsOnUsesPrefixedNames = {
      expr = (svc mon "monitoring-grafana").dependsOn;
      expected = [ "monitoring-prometheus" ];
    };
    testDependsOnOrdersTheUnits = {
      expr = {
        requires = lib.elem "podman-monitoring-prometheus.service" (
          ordered mon "podman-monitoring-grafana"
        );
        after = lib.elem "podman-monitoring-prometheus.service" mon.systemd.services.podman-monitoring-grafana.after;
        reverse = lib.elem "podman-monitoring-grafana.service" (ordered mon "podman-monitoring-prometheus");
      };
      expected = {
        requires = true;
        after = true;
        reverse = false;
      };
    };
    testDependsOnOnAnUnknownServiceFails = {
      expr =
        let
          c = evalProject {
            services.web = {
              image = "caddy:2-alpine";
              dependsOn = [ "ghost" ];
            };
          };
        in
        {
          failures = failedByPinOci c;
          named = has "p/web -> ghost" (builtins.head (failed c)).message;
        };
      expected = {
        failures = [ true ];
        named = true;
      };
    };

    # ---- composition with milestones 1 and 2 ----
    testServicesAreOrderedAfterTheirProjectResources = {
      expr = lib.sort lib.lessThan (
        lib.filter (u: has "-network-" u || has "-volume-" u) (ordered mon "podman-monitoring-prometheus")
      );
      expected = [
        "podman-network-monitoring-default.service"
        "podman-volume-monitoring_prom_data.service"
      ];
    };
    testProjectResourceUnitsExist = {
      expr = lib.sort lib.lessThan (
        lib.filter (n: has "-network-" n || has "-volume-" n) (builtins.attrNames mon.systemd.services)
      );
      expected = [
        "podman-network-monitoring-default"
        "podman-volume-monitoring_grafana_data"
        "podman-volume-monitoring_prom_data"
      ];
    };
    testServiceImagesAreLockTargets = {
      expr = builtins.attrNames mon.virtualisation.pin-oci.lockTargets;
      expected = [
        "docker.io/library/caddy:2-alpine"
        "docker.io/library/redis:7"
      ];
    };
    testUnlockedProjectImageFailsFastAndLowersNothing = {
      expr =
        let
          c = eval [
            (with' lockAbsent { })
            monitoring
          ];
        in
        {
          failures = failedByPinOci c;
          lowered = builtins.attrNames (containers c);
          targets = builtins.attrNames c.virtualisation.pin-oci.lockTargets;
        };
      expected = {
        failures = [ true ];
        lowered = [ ];
        targets = [
          "docker.io/library/caddy:2-alpine"
          "docker.io/library/redis:7"
        ];
      };
    };

    # ---- name validation and collisions ----
    # A bad volume name is also caught by milestone 2 on the generated name: only require that
    # every failure comes from pin-oci and that there is at least one.
    testInvalidNamesFailTheAssertion = {
      expr = lib.mapAttrs (_: c: failedByPinOci c != [ ] && lib.all (x: x) (failedByPinOci c)) {
        service = evalProject { services."a/b".image = "caddy:2-alpine"; };
        volume = evalProject { volumes."bad vol" = { }; };
        network = evalProject { networks."-x" = { }; };
        project = eval [
          base
          {
            virtualisation.pin-oci.projects."bad project" = {
              enable = true;
            };
          }
        ];
      };
      expected = {
        service = true;
        volume = true;
        network = true;
        project = true;
      };
    };
    testCollidingGeneratedNamesFail = {
      expr =
        let
          c = eval [
            base
            {
              virtualisation.pin-oci.projects = {
                "a-b" = {
                  enable = true;
                  services.c.image = "caddy:2-alpine";
                };
                a = {
                  enable = true;
                  services."b-c".image = "caddy:2-alpine";
                };
              };
            }
          ];
        in
        {
          failures = failedByPinOci c;
          names = has "a-b-c" (builtins.head (failed c)).message;
        };
      expected = {
        failures = [ true ];
        names = true;
      };
    };
  };
}
