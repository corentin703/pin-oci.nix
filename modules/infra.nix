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

  rootless = user: user != "root";
  userCfg = user: config.users.users.${user};

  # `inspect` rather than `exists`: Docker has no `exists` subcommand.
  # A resource owned by another user runs as that user, the way oci-containers runs a rootless
  # container: the user's account (uid, home, linger) is read from the host configuration, never
  # declared here.
  mkResourceUnit =
    {
      description,
      kind,
      name,
      createFlags,
      user,
    }:
    {
      inherit description;
      after =
        lib.optional (backend == "docker") "docker.service"
        ++ lib.optionals (rootless user && ownerReady user) (
          [ "user@${toString (userCfg user).uid}.service" ]
          ++ lib.optional (userCfg user).linger "linger-users.service"
        );
      requires =
        lib.optional (backend == "docker") "docker.service"
        ++ lib.optional (rootless user && ownerReady user) "user@${toString (userCfg user).uid}.service";
      wants = lib.optional (
        rootless user && ownerReady user && (userCfg user).linger
      ) "linger-users.service";
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      }
      // lib.optionalAttrs (rootless user) { User = user; };
      # `newuidmap` and `newgidmap` are setuid wrappers outside the store.
      path = lib.optional (rootless user) "/run/wrappers";
      environment = lib.optionalAttrs (rootless user && ownerReady user) {
        HOME = (userCfg user).home;
        XDG_RUNTIME_DIR = "/run/user/${toString (userCfg user).uid}";
      };
      script = ''
        ${bin} ${kind} inspect ${lib.escapeShellArg name} >/dev/null 2>&1 \
          || ${bin} ${kind} create ${
            lib.concatMapStrings (f: "${f} ") createFlags
          }${lib.escapeShellArg name}
      '';
    };

  networkUnits = lib.mapAttrs' (
    name: n:
    lib.nameValuePair (networkUnit name) (mkResourceUnit {
      description = "pin-oci network ${name}";
      kind = "network";
      inherit name;
      user = n.podman.user;
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
      user = v.podman.user;
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
        lib.filter (n: cfg.networks ? ${n}) (
          map networkName c.networks ++ networksFromExtraOptions c.extraOptions
        )
      )
      ++ map (n: "${volumeUnit n}.service") (
        lib.filter (n: cfg.volumes ? ${n}) (map volumeSource c.volumes)
      )
    );

  # Only containers that were actually lowered to oci-containers have a unit.
  lowered = lib.filterAttrs (
    n: _: config.virtualisation.oci-containers.containers ? ${n}
  ) cfg.containers;

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

  # The user is declared with a static uid: without it there is no /run/user/<uid> to point at.
  ownerReady = user: (config.users.users ? ${user}) && (userCfg user).uid != null;

  owners = lib.unique (
    lib.filter rootless (
      lib.mapAttrsToList (_: n: n.podman.user) cfg.networks
      ++ lib.mapAttrsToList (_: v: v.podman.user) cfg.volumes
    )
  );

  ownedResources =
    lib.mapAttrsToList (n: r: "network '${n}' (${r.podman.user})") (
      lib.filterAttrs (_: n: rootless n.podman.user) cfg.networks
    )
    ++ lib.mapAttrsToList (n: r: "volume '${n}' (${r.podman.user})") (
      lib.filterAttrs (_: v: rootless v.podman.user) cfg.volumes
    );

  # A container only sees the resources of its own user.
  ownerOf =
    kind: n: if kind == "network" then cfg.networks.${n}.podman.user else cfg.volumes.${n}.podman.user;
  ownerMismatches = lib.concatLists (
    lib.mapAttrsToList (
      cname: c:
      let
        user = c.podman.user or "root";
        nets = lib.filter (n: cfg.networks ? ${n}) (
          map networkName c.networks ++ networksFromExtraOptions c.extraOptions
        );
        vols = lib.filter (v: cfg.volumes ? ${v}) (map volumeSource c.volumes);
        wrong = kind: lib.filter (n: ownerOf kind n != user);
      in
      map (n: "${cname} (${user}) uses network '${n}' owned by ${ownerOf "network" n}") (
        wrong "network" (lib.unique nets)
      )
      ++ map (v: "${cname} (${user}) uses volume '${v}' owned by ${ownerOf "volume" v}") (
        wrong "volume" (lib.unique vols)
      )
    ) lowered
  );

  notReady = lib.filter (u: !ownerReady u) owners;
  withoutLinger = lib.filter (u: ownerReady u && !(userCfg u).linger) owners;

  invalidNames = lib.filter (n: !validName n) (
    builtins.attrNames cfg.networks ++ builtins.attrNames cfg.volumes
  );

  userOption =
    kind:
    lib.mkOption {
      type = lib.types.str;
      default = "root";
      example = "oci-app";
      description = ''
        The user whose Podman owns this ${kind}, like `containers.<name>.podman.user` for a
        container: a rootless container only sees the resources of its own user. The account
        (static `uid`, `home`, `linger`) is the one in `users.users`; pin-oci never declares it.
        Podman only.
      '';
    };
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
            podman.user = userOption "network";
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
          options.podman.user = userOption "volume";
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
      {
        assertion = backend == "podman" || ownedResources == [ ];
        message = ''
          [pin-oci] podman.user is only for the podman backend, but the backend is '${backend}':
            - ${lib.concatStringsSep "\n  - " ownedResources}
          Remove podman.user, or set virtualisation.oci-containers.backend = "podman".
        '';
      }
      {
        assertion = backend != "podman" || notReady == [ ];
        message = ''
          [pin-oci] The owner of a network or volume must be declared in users.users with a static uid:
            - ${lib.concatStringsSep "\n  - " notReady}
          Declare it, for example users.users.<name> = { isSystemUser = true; uid = 2001; ... };
        '';
      }
      {
        assertion = ownerMismatches == [ ];
        message = ''
          [pin-oci] A rootless container only sees the networks and volumes of its own user:
            - ${lib.concatStringsSep "\n  - " ownerMismatches}
          Give the resource and the container the same podman.user.
        '';
      }
    ];

    warnings = map (
      u:
      "[pin-oci] User ${u} owns a network or volume but lingering is turned off for it: its Podman only works while it is logged in. Set users.users.${u}.linger = true."
    ) withoutLinger;

    systemd.services = lib.mkMerge [
      networkUnits
      volumeUnits
      containerOrdering
    ];
  };
}
