{ lib }:

rec {
  # Docker/Podman resource name rule; also safe inside a unit name.
  validName = name: builtins.match "[a-zA-Z0-9][a-zA-Z0-9_.-]*" name != null;

  # Network a `--network` value designates:
  #   app-net                          -> app-net
  #   app-net:alias=db                 -> app-net     (Podman)
  #   name=app-net,alias=db            -> app-net     (Docker)
  networkName =
    value:
    let
      csv = builtins.match "(.*,)?name=([^,]*).*" value;
    in
    if csv != null then builtins.elemAt csv 1 else builtins.head (lib.splitString ":" value);

  # Source of a `volumes` entry "src:dst[:opts]".
  volumeSource = entry: builtins.head (lib.splitString ":" entry);

  # Networks from `--network=x`, `--net=x` and the two-element form `--network x`.
  networksFromExtraOptions =
    opts:
    lib.concatLists (
      lib.imap0 (
        i: opt:
        let
          m = builtins.match "--net(work)?=(.*)" opt;
        in
        if m != null then
          [ (networkName (builtins.elemAt m 1)) ]
        else if (opt == "--network" || opt == "--net") && i + 1 < builtins.length opts then
          [ (networkName (builtins.elemAt opts (i + 1))) ]
        else
          [ ]
      ) opts
    );
}
