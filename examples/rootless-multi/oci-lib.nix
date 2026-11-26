# YOURS, not pin-oci's. Plain Nix, no module: the one table of the rootless users (name -> uid) and
# everything derived from it, so that a uid, a path or a subuid range is written once.
# Adapted from a production host.
let
  baseId = 2000;
  subIdBase = 1000000;
  subIdCount = 65536;

  # One user per project: its containers, networks and volumes are private to it. A new project
  # is a new line here (and its `podman.user` in containers.nix); users and directories follow.
  uids = {
    blog = 2001;
    tools = 2002;
  };

  index = name: uids.${name} - baseId;
in
rec {
  inherit uids subIdCount;

  userName = name: "oci-${name}";
  uid = name: toString uids.${name};

  # <rootDir>/home: the home of the user (Podman configuration only).
  # <rootDir>/data: everything that is mounted into the containers.
  rootDir = name: "/srv/oci/${name}";
  homeDir = name: "${rootDir name}/home";
  dataDir = name: "${rootDir name}/data";

  # Image storage of the user, kept out of /srv (not worth backing up).
  storageDir = name: "/var/lib/oci-storage/${userName name}";

  # The ranges of two users never overlap: user n owns [start n, start n + subIdCount).
  subIdStart = name: subIdBase + (index name - 1) * subIdCount;

  # The uid on the host of an id inside a container. Rootless: 0 (root) is the user itself, and
  # n >= 1 is in its subuid range. `hostId "blog" 70` is `postgres` of the Alpine image, which
  # owns the directory that the database writes to.
  hostId = name: n: if n == 0 then uids.${name} else subIdStart name + n - 1;
}
