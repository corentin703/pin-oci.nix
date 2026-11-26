# YOURS, not pin-oci's. Everything rootless Podman needs from the host, for every user of
# oci-lib.nix: uid, subuid/subgid range, lingering, a home and a storage directory.
# pin-oci only reads `uid`, `home` and `linger` of these accounts (to run a project's networks
# and volumes as that user); it declares none of it. Adapted from a production host.
{ lib, pkgs, ... }:
let
  oci = import ./oci-lib.nix;
  names = builtins.attrNames oci.uids;

  storageConf =
    name:
    pkgs.writeText "storage-${name}.conf" ''
      [storage]
      driver = "overlay"
      graphroot = "${oci.storageDir name}"
    '';
in
{
  users.groups = lib.genAttrs (map oci.userName names) (group: {
    gid = oci.uids.${lib.removePrefix "oci-" group};
  });

  users.users = lib.genAttrs (map oci.userName names) (
    user:
    let
      name = lib.removePrefix "oci-" user;
    in
    {
      isSystemUser = true;
      uid = oci.uids.${name};
      group = user;
      description = "OCI ${name}";
      home = oci.homeDir name;
      createHome = false;
      shell = "${pkgs.shadow}/bin/nologin";
      # Podman rootless, started by systemd, needs the user manager of the user to be running.
      linger = true;
      autoSubUidGidRange = false;
      subUidRanges = [
        {
          startUid = oci.subIdStart name;
          count = oci.subIdCount;
        }
      ];
      subGidRanges = [
        {
          startGid = oci.subIdStart name;
          count = oci.subIdCount;
        }
      ];
    }
  );

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
      user = oci.userName name;
    in
    [
      "d ${oci.rootDir name} 0755 root root -"
      "d ${oci.homeDir name} 0700 ${user} ${user} -"
      "d ${oci.homeDir name}/.config 0700 ${user} ${user} -"
      "d ${oci.homeDir name}/.config/containers 0700 ${user} ${user} -"
      "L+ ${oci.homeDir name}/.config/containers/storage.conf - - - - ${storageConf name}"
      # Owned by root: what a container writes goes in a directory below, owned by a hostId.
      "d ${oci.dataDir name} 0755 root root -"
      "d ${oci.storageDir name} 0700 ${user} ${user} -"
    ]
  ) names;
}
