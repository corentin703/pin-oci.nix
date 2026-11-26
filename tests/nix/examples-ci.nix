{
  pkgs,
  pin-oci,
  version,
}:

# The CI examples are parsed, call commands the CLI really has, and pin the CLI to the release
# being built: a version bump that forgets them fails here instead of shipping a stale tag.
pkgs.runCommand "pin-oci-examples-ci"
  {
    nativeBuildInputs = [
      pkgs.yq-go
      pin-oci
    ];
    src = pkgs.lib.fileset.toSource {
      root = ../../examples/ci;
      fileset = ../../examples/ci;
    };
  }
  ''
    cd $src
    for f in *.yml; do
      yq '.jobs' "$f" > /dev/null || { echo "$f is not valid YAML" >&2; exit 1; }
      grep -q 'nix run github:corentin703/pin-oci.nix/v${version} -- lock ' "$f" \
        || { echo "$f does not pin the CLI to v${version}" >&2; exit 1; }
    done
    grep -q -- '-- lock check --flake' github-check.yml
    grep -q -- '-- lock check --flake' forgejo-check.yml
    grep -q -- '-- lock update --flake' github-update.yml
    grep -q -- '-- lock update --flake' forgejo-update.yml
    pin-oci lock check --help > /dev/null
    pin-oci lock update --help > /dev/null
    touch $out
  ''
