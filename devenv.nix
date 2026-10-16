{ pkgs, ... }:

{
  # Default "nixpkgs" channel: cargo, rustc, clippy, rustfmt, rust-analyzer.
  languages.rust.enable = true;

  # Same tooling as `devShells.default` in flake.nix, plus the runtime dependency of the CLI.
  packages = [
    pkgs.skopeo
    pkgs.nixfmt
  ];
}
