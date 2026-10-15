{ pkgs, ... }:

{
  # Default "nixpkgs" channel: cargo, rustc, clippy, rustfmt, rust-analyzer.
  languages.rust.enable = true;

  # Runtime dependency of the pin-oci CLI.
  packages = [ pkgs.skopeo ];
}
