{
  config,
  options,
  lib,
  ...
}:

# Compose-style projects, compiled down to virtualisation.pin-oci.{containers,networks,volumes}.
let
  cfg = config.virtualisation.pin-oci;
  backend = config.virtualisation.oci-containers.backend;

  inherit (import ./resources.nix { inherit lib; })
    validName
    renameNetworkValue
    renameNetworkOptions
    renameVolumeEntry
    ;

  serviceModule = import ./container-module.nix { inherit options; };

  networkModule =
    { name, ... }:
    {
      options = {
        enable = lib.mkOption {
          type = lib.types.bool;
          # Declaring a network creates it; the implicit one has to be asked for.
          default = name != "default";
          defaultText = lib.literalExpression ''name != "default"'';
          description = "Whether the network is created.";
        };
        external = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = ''
            Refer to a global virtualisation.pin-oci.networks entry: no prefix, nothing generated.
          '';
        };
        subnet = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          description = "Subnet in CIDR notation.";
        };
        internal = lib.mkOption {
          type = lib.types.bool;
          default = false;
          description = "Restrict external access to the network.";
        };
      };
    };

  volumeModule = {
    options = {
      external = lib.mkOption {
        type = lib.types.bool;
        default = false;
        description = ''
          Refer to a global virtualisation.pin-oci.volumes entry: no prefix, nothing generated.
        '';
      };
      driver = lib.mkOption {
        type = lib.types.str;
        default = "local";
        description = "Volume driver.";
      };
    };
  };

  projectModule = {
    options = {
      enable = lib.mkEnableOption "this project";

      networks = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule networkModule);
        default = { };
        description = ''
          Project networks. `default` is the implicit network every service joins;
          enable it with `networks.default.enable = true`.
        '';
      };

      volumes = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule volumeModule);
        default = { };
        description = "Project volumes.";
      };

      services = lib.mkOption {
        type = lib.types.attrsOf (lib.types.submodule serviceModule);
        default = { };
        description = "Services of the project, with the options of a container.";
      };
    };
  };

  projects = lib.filterAttrs (_: p: p.enable) cfg.projects;

  prefixed = project: name: "${project}-${name}";
  prefixedVolume = project: name: "${project}_${name}";

  # Resources this project creates (and therefore prefixes).
  ownNetworks = pc: lib.filterAttrs (_: n: n.enable && !n.external) pc.networks;
  ownVolumes = pc: lib.filterAttrs (_: v: !v.external) pc.volumes;

  # Default network entry: the service name becomes a DNS alias on it.
  defaultNetworkEntry =
    project: service:
    if backend == "docker" then
      "name=${prefixed project "default"},alias=${service}"
    else
      "${prefixed project "default"}:alias=${service}";

  lowerService =
    project: pc: service: svc:
    let
      renameNetwork = n: if (ownNetworks pc) ? ${n} then prefixed project n else n;
      renameVolume = v: if (ownVolumes pc) ? ${v} then prefixedVolume project v else v;
      joinsDefault = (pc.networks.default.enable or false);
    in
    # `serviceName` defaults to `<backend>-<name>`: drop the default so it follows the prefixed
    # name, keep an explicit one.
    (removeAttrs svc (
      [ "_module" ] ++ lib.optional (svc.serviceName == "${backend}-${service}") "serviceName"
    ))
    // {
      # Unknown names are reported by an assertion; passing them on would fail earlier, inside
      # oci-containers, with an obscure message.
      dependsOn = map (prefixed project) (lib.filter (d: pc.services ? ${d}) svc.dependsOn);
      networks =
        lib.optional joinsDefault (defaultNetworkEntry project service)
        ++ map (renameNetworkValue renameNetwork) svc.networks;
      volumes = map (renameVolumeEntry renameVolume) svc.volumes;
      extraOptions = renameNetworkOptions renameNetwork svc.extraOptions;
    };

  containers = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (
      service: svc: lib.nameValuePair (prefixed project service) (lowerService project pc service svc)
    ) pc.services
  ) projects;

  networks = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (n: nc: lib.nameValuePair (prefixed project n) { inherit (nc) subnet internal; }) (
      ownNetworks pc
    )
  ) projects;

  volumes = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (v: vc: lib.nameValuePair (prefixedVolume project v) { inherit (vc) driver; }) (
      ownVolumes pc
    )
  ) projects;

  # ---- assertions ----
  flat = f: lib.concatLists (lib.mapAttrsToList f projects);

  invalidNames = lib.filter (n: !validName n) (
    lib.attrNames projects
    ++ flat (
      _: pc:
      lib.attrNames pc.services
      ++ lib.attrNames pc.networks
      ++ lib.attrNames pc.volumes
    )
  );

  duplicates =
    names: lib.unique (lib.filter (n: lib.count (m: m == n) names > 1) names);

  duplicateNames =
    duplicates (flat (project: pc: map (prefixed project) (lib.attrNames pc.services)))
    ++ duplicates (flat (project: pc: map (prefixed project) (lib.attrNames (ownNetworks pc))))
    ++ duplicates (flat (project: pc: map (prefixedVolume project) (lib.attrNames (ownVolumes pc))));

  externalDefault = lib.attrNames (lib.filterAttrs (_: pc: pc.networks.default.external or false) projects);

  missingExternals = flat (
    project: pc:
    map (n: "${project}: network '${n}'") (
      lib.filter (n: n != "default" && !(cfg.networks ? ${n})) (
        lib.attrNames (lib.filterAttrs (_: n: n.enable && n.external) pc.networks)
      )
    )
    ++ map (v: "${project}: volume '${v}'") (
      lib.filter (v: !(cfg.volumes ? ${v})) (lib.attrNames (lib.filterAttrs (_: v: v.external) pc.volumes))
    )
  );

  unknownDependencies = flat (
    project: pc:
    lib.concatLists (
      lib.mapAttrsToList (
        service: svc:
        map (d: "${project}/${service} -> ${d}") (lib.filter (d: !(pc.services ? ${d})) svc.dependsOn)
      ) pc.services
    )
  );

  list = items: lib.concatMapStringsSep "\n  - " (i: i) items;
in
{
  options.virtualisation.pin-oci.projects = lib.mkOption {
    type = lib.types.attrsOf (lib.types.submodule projectModule);
    default = { };
    description = "Compose-style projects: services sharing a namespace, networks and volumes.";
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = invalidNames == [ ];
        message = ''
          [pin-oci] Invalid project/service/network/volume name(s):
            - ${list invalidNames}
          Names must match [a-zA-Z0-9][a-zA-Z0-9_.-]*
        '';
      }
      {
        assertion = duplicateNames == [ ];
        message = ''
          [pin-oci] Several projects generate the same name(s):
            - ${list duplicateNames}
        '';
      }
      {
        assertion = externalDefault == [ ];
        message = ''
          [pin-oci] The default network cannot be external (project(s): ${lib.concatStringsSep ", " externalDefault})
        '';
      }
      {
        assertion = missingExternals == [ ];
        message = ''
          [pin-oci] External resources missing from virtualisation.pin-oci.{networks,volumes}:
            - ${list missingExternals}
        '';
      }
      {
        assertion = unknownDependencies == [ ];
        message = ''
          [pin-oci] dependsOn names a service that is not in the project:
            - ${list unknownDependencies}
        '';
      }
    ];

    virtualisation.pin-oci = {
      containers = containers;
      networks = networks;
      volumes = volumes;
    };
  };
}
