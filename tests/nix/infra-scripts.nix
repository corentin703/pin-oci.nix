{
  pkgs,
  nixpkgs,
  module,
}:

# Runs the *generated* network/volume unit scripts against a fake stateful backend:
# idempotence, flags, recreation of a removed resource and failure propagation.
let
  inherit (import ./common.nix { inherit pkgs nixpkgs module; }) lib eval with' lockOk;

  config = eval [
    (with' lockOk { })
    {
      virtualisation.pin-oci = {
        networks.app-net = {
          subnet = "10.89.1.0/24";
          internal = true;
        };
        volumes.pg = { };
      };
    }
  ];

  # Behaves like `podman`: resources are files under $STATE, every call is logged.
  fakeBackend = pkgs.writeShellScript "fake-backend" ''
    echo "$*" >> "$STATE/log"
    kind=$1 verb=$2
    case "$verb" in
      inspect) [ -e "$STATE/$kind-$3" ] ;;
      create)
        [ -z "''${FAIL_CREATE:-}" ] || { echo "create failed" >&2; exit 1; }
        for last; do :; done
        touch "$STATE/$kind-$last"
        ;;
      *) echo "unexpected call: $*" >&2; exit 64 ;;
    esac
  '';

  realBackend = builtins.unsafeDiscardStringContext "${pkgs.podman}/bin/podman";

  scriptOf =
    name:
    pkgs.writeText "${name}.sh" (
      builtins.replaceStrings [ realBackend ] [ "${fakeBackend}" ] (
        builtins.unsafeDiscardStringContext config.systemd.services.${name}.script
      )
    );

  network = scriptOf "podman-network-app-net";
  volume = scriptOf "podman-volume-pg";
in
pkgs.runCommand "pin-oci-infra-scripts" { nativeBuildInputs = [ pkgs.bash ]; } ''
  export STATE=$TMPDIR/state
  mkdir -p "$STATE"
  creates() { grep -c ' create ' "$STATE/log"; }

  # Fresh host: both resources are created, with the declared flags.
  bash -eu ${network}
  bash -eu ${volume}
  test "$(creates)" = 2
  grep -qx 'network create --subnet 10.89.1.0/24 --internal app-net' "$STATE/log"
  grep -qx 'volume create --driver local pg' "$STATE/log"

  # Idempotent: running again creates nothing.
  bash -eu ${network}
  bash -eu ${volume}
  test "$(creates)" = 2

  # A resource removed behind our back is recreated.
  rm "$STATE/network-app-net"
  bash -eu ${network}
  test "$(creates)" = 3
  test -e "$STATE/network-app-net"

  # A failing create fails the unit instead of being swallowed.
  rm "$STATE/volume-pg"
  if FAIL_CREATE=1 bash -eu ${volume}; then
    echo "create failure was ignored" >&2
    exit 1
  fi

  # An existing resource never reaches create, even if create would fail.
  FAIL_CREATE=1 bash -eu ${network}

  echo "infra scripts ok"
  touch $out
''
