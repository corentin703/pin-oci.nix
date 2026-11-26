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

  # A project's user is the default owner of what the project creates; each one can override it.
  userOption =
    projectUser:
    lib.mkOption {
      type = lib.types.str;
      default = if projectUser == null then "root" else projectUser;
      defaultText = lib.literalExpression ''the project's podman.user, else "root"'';
      description = "The user whose Podman owns this resource (see `virtualisation.pin-oci.networks.<name>.podman.user`).";
    };

  networkModule =
    projectUser:
    { name, ... }:
    {
      options = {
        podman.user = userOption projectUser;
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

  volumeModule = projectUser: {
    options = {
      podman.user = userOption projectUser;
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

  projectModule =
    { config, ... }:
    {
      options = {
        enable = lib.mkEnableOption "this project";

        podman.user = lib.mkOption {
          type = lib.types.nullOr lib.types.str;
          default = null;
          example = "oci-app";
          description = ''
            The user that manages the project, for Podman. It is the default `podman.user` of every
            service and of every network and volume the project creates; each of them can set its own.
            The account is the one in `users.users`: the project never declares it. `null` leaves the
            defaults (`root`).
          '';
        };

        networks = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule (networkModule config.podman.user));
          default = { };
          description = ''
            Project networks. `default` is the implicit network every service joins;
            enable it with `networks.default.enable = true`.
          '';
        };

        volumes = lib.mkOption {
          type = lib.types.attrsOf (lib.types.submodule (volumeModule config.podman.user));
          default = { };
          description = "Project volumes.";
        };

        services = lib.mkOption {
          type = lib.types.attrsOf (
            lib.types.submodule [
              serviceModule
              {
                # Not even an empty `podman` when the project has no user: oci-containers refuses the
                # option with the docker backend.
                podman = lib.mkIf (config.podman.user != null) { user = lib.mkDefault config.podman.user; };
              }
            ]
          );
          default = { };
          description = "Services of the project, with the options of a container.";
        };

        followLockFile = lib.mkOption {
          type = lib.types.nullOr lib.types.path;
          default = null;
          example = lib.literalExpression "./pin-oci.lock";
          description = ''
            Lockfile the images of this project follow. `null` (the default) is the host's
            `virtualisation.pin-oci.lockFile`. A flake that ships a project sets it (with
            `lib.mkDefault`) to its own `pin-oci.lock`, which the host then uses as is: the project
            keeps the digests its author locked, and the host's `pin-oci lock` does not touch them.
            The host takes the images over by setting it to `null` (or to its own file): they are
            then locked with the host's.
          '';
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
      lockFile = pc.followLockFile;
    };

  containers = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (
      service: svc: lib.nameValuePair (prefixed project service) (lowerService project pc service svc)
    ) pc.services
  ) projects;

  networks = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (
      n: nc: lib.nameValuePair (prefixed project n) { inherit (nc) subnet internal podman; }
    ) (ownNetworks pc)
  ) projects;

  volumes = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (
      v: vc: lib.nameValuePair (prefixedVolume project v) { inherit (vc) driver podman; }
    ) (ownVolumes pc)
  ) projects;

  # Unit of a service: an explicit `serviceName` wins over the `<backend>-<container>` default.
  unitName =
    project: service: svc:
    if svc.serviceName == "${backend}-${service}" then
      "${backend}-${prefixed project service}"
    else
      svc.serviceName;

  # One `<backend>-<project>.target` per project: start/stop/restart it to act on every service.
  projectTargets = lib.mapAttrs' (
    project: pc:
    lib.nameValuePair "${backend}-${project}" {
      description = "pin-oci project ${project}";
      wantedBy = [ "multi-user.target" ];
      wants = lib.mapAttrsToList (s: svc: "${unitName project s svc}.service") pc.services;
    }
  ) projects;

  serviceUnits = lib.concatMapAttrs (
    project: pc:
    lib.mapAttrs' (
      s: svc:
      lib.nameValuePair (unitName project s svc) {
        partOf = [ "${backend}-${project}.target" ];
      }
    ) pc.services
  ) projects;

  # ---- assertions ----
  flat = f: lib.concatLists (lib.mapAttrsToList f projects);

  invalidNames = lib.filter (n: !validName n) (
    lib.attrNames projects
    ++ flat (_: pc: lib.attrNames pc.services ++ lib.attrNames pc.networks ++ lib.attrNames pc.volumes)
  );

  duplicates = names: lib.unique (lib.filter (n: lib.count (m: m == n) names > 1) names);

  duplicateNames =
    duplicates (flat (project: pc: map (prefixed project) (lib.attrNames pc.services)))
    ++ duplicates (flat (project: pc: map (prefixed project) (lib.attrNames (ownNetworks pc))))
    ++ duplicates (flat (project: pc: map (prefixedVolume project) (lib.attrNames (ownVolumes pc))));

  externalDefault = lib.attrNames (
    lib.filterAttrs (_: pc: pc.networks.default.external or false) projects
  );

  missingExternals = flat (
    project: pc:
    map (n: "${project}: network '${n}'") (
      lib.filter (n: n != "default" && !(cfg.networks ? ${n})) (
        lib.attrNames (lib.filterAttrs (_: n: n.enable && n.external) pc.networks)
      )
    )
    ++ map (v: "${project}: volume '${v}'") (
      lib.filter (v: !(cfg.volumes ? ${v})) (
        lib.attrNames (lib.filterAttrs (_: v: v.external) pc.volumes)
      )
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

    systemd.targets = projectTargets;
    systemd.services = serviceUnits;
  };
}
