{ pkgs, pin-oci }:

# The package ships the completion scripts, and they describe the CLI the package contains:
# each file exists, mentions the modes, and equals what the packaged binary prints today.
pkgs.runCommand "pin-oci-completions-tests"
  {
    nativeBuildInputs = [ pkgs.diffutils ];
  }
  ''
    check() {
      file="$1"; shell="$2"; shift 2
      [ -s "$file" ] || { echo "missing or empty: $file" >&2; exit 1; }
      for word in "$@"; do
        grep -q -e "$word" "$file" || { echo "$file does not mention '$word'" >&2; exit 1; }
      done
      ${pin-oci}/bin/pin-oci completions "$shell" | diff - "$file" >/dev/null \
        || { echo "$file differs from 'pin-oci completions $shell'" >&2; exit 1; }
    }

    check ${pin-oci}/share/bash-completion/completions/pin-oci.bash bash update list check
    check ${pin-oci}/share/zsh/site-functions/_pin-oci zsh update list check
    check ${pin-oci}/share/fish/vendor_completions.d/pin-oci.fish fish update list check

    echo "completion scripts installed and up to date"
    touch $out
  ''
