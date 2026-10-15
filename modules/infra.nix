{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.virtualisation.pin-oci;
  backend = config.virtualisation.oci-containers.backend;
  bin = "${pkgs.${backend}}/bin/${backend}";

  inherit (import ./resources.nix { inherit lib; })
    validName
    networkName
    volumeSource
    networksFromExtraOptions
    ;

  networkUnit = name: "${backend}-network-${name}";
  volumeUnit = name: "${backend}-volume-${name}";

  # `inspect` rather than `exists`: Docker has no `exists` subcommand.
  mkResourceUnit =
    {
      description,
      kind,
      name,
      createFlags,
    }:
    {
      inherit description;
      after = lib.optional (backend == "docker") "docker.service";
      requires = lib.optional (backend == "docker") "docker.service";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        ${bin} ${kind} inspect ${lib.escapeShellArg name} >/dev/null 2>&1 \
          || ${bin} ${kind} create ${lib.concatMapStrings (f: "${f} ") createFlags}${lib.escapeShellArg name}
      '';
    };

  networkUnits = lib.mapAttrs' (
    name: n:
    lib.nameValuePair (networkUnit name) (mkResourceUnit {
      description = "pin-oci network ${name}";
      kind = "network";
      inherit name;
      createFlags =
        lib.optionals (n.subnet != null) [
          "--subnet"
          (lib.escapeShellArg n.subnet)
        ]
        ++ lib.optional n.internal "--internal";
    })
  ) cfg.networks;

  volumeUnits = lib.mapAttrs' (
    name: v:
    lib.nameValuePair (volumeUnit name) (mkResourceUnit {
      description = "pin-oci volume ${name}";
      kind = "volume";
      inherit name;
      createFlags = [
        "--driver"
        (lib.escapeShellArg v.driver)
      ];
    })
  ) cfg.volumes;

  # Declared resources a container references, as unit names.
  dependenciesOf =
    c:
    lib.unique (
      map (n: "${networkUnit n}.service") (
        lib.filter (n: cfg.networks ? ${n}) (map networkName c.networks ++ networksFromExtraOptions c.extraOptions)
      )
      ++ map (n: "${volumeUnit n}.service") (lib.filter (n: cfg.volumes ? ${n}) (map volumeSource c.volumes))
    );

  # Only containers that were actually lowered to oci-containers have a unit.
  lowered = lib.filterAttrs (n: _: config.virtualisation.oci-containers.containers ? ${n}) cfg.containers;

  containerOrdering = lib.mapAttrs' (
    _: c:
    let
      deps = dependenciesOf c;
    in
    lib.nameValuePair c.serviceName (
      lib.mkIf (deps != [ ]) {
        after = deps;
        requires = deps;
      }
    )
  ) lowered;

  invalidNames = lib.filter (n: !validName n) (
    builtins.attrNames cfg.networks ++ builtins.attrNames cfg.volumes
  );
in
{
  options.virtualisation.pin-oci = {
    networks = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options = {
            subnet = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "10.89.1.0/24";
              description = "Subnet in CIDR notation.";
            };
            internal = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = "Restrict external access to the network (no Internet gateway).";
            };
          };
        }
      );
      default = { };
      description = "Networks created before the containers that use them.";
    };

    volumes = lib.mkOption {
      type = lib.types.attrsOf (
        lib.types.submodule {
          options.driver = lib.mkOption {
            type = lib.types.str;
            default = "local";
            description = "Volume driver.";
          };
        }
      );
      default = { };
      description = "Named volumes created before the containers that use them.";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = invalidNames == [ ];
        message = ''
          [pin-oci] Invalid network/volume name(s): ${lib.concatStringsSep ", " invalidNames}
          Names must match [a-zA-Z0-9][a-zA-Z0-9_.-]*
        '';
      }
    ];

    systemd.services = lib.mkMerge [
      networkUnits
      volumeUnits
      containerOrdering
    ];
  };
}
