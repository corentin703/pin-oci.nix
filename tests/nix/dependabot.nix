{ pkgs }:

# Every ecosystem that Dependabot is told to watch has the manifest it reads, so a renamed or
# removed file does not leave a silent entry. The check is run on an empty tree too, where it must
# fail: a check that cannot fail proves nothing.
pkgs.runCommand "pin-oci-dependabot"
  {
    nativeBuildInputs = [ pkgs.yq-go ];
    src = pkgs.lib.fileset.toSource {
      root = ../..;
      fileset = pkgs.lib.fileset.unions [
        ../../.github
        ../../Cargo.toml
        ../../Cargo.lock
        ../../flake.nix
        ../../flake.lock
      ];
    };
  }
  ''
    check() {
      local root=$1 cfg=$1/.github/dependabot.yml ecosystems
      ecosystems=$(yq '.updates[].package-ecosystem' "$cfg") || return 1
      [ -n "$ecosystems" ] || return 1
      for e in $ecosystems; do
        case "$e" in
          cargo) [ -f "$root/Cargo.toml" ] && [ -f "$root/Cargo.lock" ] ;;
          github-actions) ls "$root"/.github/workflows/*.yml > /dev/null ;;
          nix) [ -f "$root/flake.nix" ] && [ -f "$root/flake.lock" ] ;;
          *) echo "unknown ecosystem $e" >&2; false ;;
        esac || { echo "ecosystem $e has no manifest" >&2; return 1; }
      done
    }

    check $src
    mkdir -p empty/.github
    cp $src/.github/dependabot.yml empty/.github/
    if check empty 2> /dev/null; then
      echo "the check passes on a tree without manifests" >&2
      exit 1
    fi
    touch $out
  ''
