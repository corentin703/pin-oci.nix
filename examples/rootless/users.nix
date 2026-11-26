# YOURS, not pin-oci's. Everything rootless Podman needs from the host: one system user per
# project, with its uid, its subuid/subgid range, lingering, a home and a storage directory.
# pin-oci only reads `uid`, `home` and `linger` of these accounts (to run a project's networks
# and volumes as that user); it declares none of it. Adapted from a production host.
{ lib, pkgs, ... }:
let
  # name -> uid. One user per project: its containers, networks and volumes are private to it.
  projects = {
    web = 2001;
  };

  userName = name: "oci-${name}";
  rootDir = name: "/srv/oci/${name}";
  homeDir = name: "${rootDir name}/home";
  dataDir = name: "${rootDir name}/data";
  # Image storage of the user, kept out of /srv (not worth backing up).
  storageDir = name: "/var/lib/oci-storage/${userName name}";

  # A range of 65536 subordinate ids per user, computed so the owner of a file written by a
  # container is known in advance.
  subIdStart = name: 1000000 + (projects.${name} - 2001) * 65536;

  storageConf =
    name:
    pkgs.writeText "storage-${name}.conf" ''
      [storage]
      driver = "overlay"
      graphroot = "${storageDir name}"
    '';

  names = builtins.attrNames projects;
in
{
  users.groups = lib.mapAttrs' (name: uid: lib.nameValuePair (userName name) { gid = uid; }) projects;

  users.users = lib.mapAttrs' (
    name: uid:
    lib.nameValuePair (userName name) {
      isSystemUser = true;
      inherit uid;
      group = userName name;
      home = homeDir name;
      createHome = false;
      shell = "${pkgs.shadow}/bin/nologin";
      # Podman rootless, started by systemd, needs the user manager of the user to be running.
      linger = true;
      autoSubUidGidRange = false;
      subUidRanges = [
        {
          startUid = subIdStart name;
          count = 65536;
        }
      ];
      subGidRanges = [
        {
          startGid = subIdStart name;
          count = 65536;
        }
      ];
    }
  ) projects;

  # Without it this oneshot runs again at each container start, and several containers starting
  # together exceed systemd's start limit.
  systemd.services.linger-users.serviceConfig.RemainAfterExit = true;

  systemd.tmpfiles.rules = [
    "d /srv/oci 0755 root root -"
    "d /var/lib/oci-storage 0755 root root -"
  ]
  ++ lib.concatMap (
    name:
    let
      user = userName name;
    in
    [
      "d ${rootDir name} 0755 root root -"
      "d ${homeDir name} 0700 ${user} ${user} -"
      "d ${homeDir name}/.config 0700 ${user} ${user} -"
      "d ${homeDir name}/.config/containers 0700 ${user} ${user} -"
      "L+ ${homeDir name}/.config/containers/storage.conf - - - - ${storageConf name}"
      "d ${dataDir name} 0755 ${user} ${user} -"
      "d ${storageDir name} 0700 ${user} ${user} -"
    ]
  ) names;
}
