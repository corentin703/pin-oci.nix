{
  description = "Deterministic OCI image pinning for NixOS";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      nixosModules.default = import ./modules/default.nix;

      packages = forAllSystems (pkgs: rec {
        pin-oci = pkgs.rustPlatform.buildRustPackage {
          pname = "pin-oci";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.makeWrapper ];
          # Runtime dependency: registry access goes through skopeo.
          postInstall = ''
            wrapProgram $out/bin/pin-oci --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.skopeo ]}
          '';
          meta.mainProgram = "pin-oci";
        };
        default = pin-oci;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = [ pkgs.cargo pkgs.rustc pkgs.clippy pkgs.skopeo ];
        };
      });

      # buildRustPackage runs `cargo test` in its check phase.
      checks = forAllSystems (pkgs: {
        inherit (self.packages.${pkgs.stdenv.hostPlatform.system}) pin-oci;
        module-tests = import ./tests/nix {
          inherit pkgs nixpkgs;
          module = self.nixosModules.default;
        };
        vm-infra = import ./tests/nix/vm-infra.nix {
          inherit pkgs;
          module = self.nixosModules.default;
        };
        vm-projects = import ./tests/nix/vm-projects.nix {
          inherit pkgs;
          module = self.nixosModules.default;
        };
        infra-scripts = import ./tests/nix/infra-scripts.nix {
          inherit pkgs nixpkgs;
          module = self.nixosModules.default;
        };
      });
    };
}
