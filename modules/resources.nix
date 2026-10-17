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
  volumeSource =
    entry: builtins.head (lib.splitString ":" (builtins.unsafeDiscardStringContext entry));

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

  # Replaces the network name inside a `--network` value, keeping the rest of the value.
  renameNetworkValue =
    rename: value:
    let
      csv = builtins.match "(.*,)?name=([^,]*)(.*)" value;
      before =
        if csv == null then "" else (if builtins.elemAt csv 0 == null then "" else builtins.elemAt csv 0);
      name = networkName value;
    in
    if csv != null then
      "${before}name=${rename (builtins.elemAt csv 1)}${builtins.elemAt csv 2}"
    else
      "${rename name}${lib.removePrefix name value}";

  # Same for the network forms of `extraOptions` (`--network=x`, `--net=x`, `--network x`).
  renameNetworkOptions =
    rename: opts:
    lib.imap0 (
      i: opt:
      let
        m = builtins.match "(--net(work)?=)(.*)" opt;
        previous = if i > 0 then builtins.elemAt opts (i - 1) else "";
      in
      if m != null then
        "${builtins.elemAt m 0}${renameNetworkValue rename (builtins.elemAt m 2)}"
      else if previous == "--network" || previous == "--net" then
        renameNetworkValue rename opt
      else
        opt
    ) opts;

  # Rewrites the source of a `volumes` entry, keeping `:dst[:opts]`.
  renameVolumeEntry =
    rename: entry:
    let
      src = volumeSource entry;
    in
    "${rename src}${lib.removePrefix src entry}";
}
