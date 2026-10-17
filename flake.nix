{
  description = "Deterministic OCI image pinning for NixOS";

  inputs.nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";

  # Binary cache for packages.<system>.pin-oci (pushed by CI). Mirrored in the README.
  nixConfig = {
    extra-substituters = [ "https://pin-oci.cachix.org" ];
    extra-trusted-public-keys = [ "pin-oci.cachix.org-1:rnJNoJhMLyEkTckqap7U3sTpic1xvACQlpSNqFHTPys=" ];
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      nixosModules.default = import ./modules/default.nix;

      # Opt-in: `nixpkgs.overlays = [ pin-oci.overlays.default ];` gives `pkgs.pin-oci`.
      # Reuses `packages.<system>.pin-oci` as is, so the binary cache still applies.
      overlays.default = final: _prev: {
        pin-oci = self.packages.${final.stdenv.hostPlatform.system}.pin-oci;
      };

      packages = forAllSystems (pkgs: rec {
        pin-oci = pkgs.rustPlatform.buildRustPackage {
          pname = "pin-oci";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          # Only what cargo reads: docs and Nix files do not invalidate the build.
          src = pkgs.lib.fileset.toSource {
            root = ./.;
            fileset = pkgs.lib.fileset.unions [
              ./Cargo.toml
              ./Cargo.lock
              ./src
              ./tests/fixtures
              (pkgs.lib.fileset.fileFilter (file: file.hasExt "rs") ./tests)
            ];
          };
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
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.pin-oci ];
          packages = [
            pkgs.clippy
            pkgs.rustfmt
            pkgs.rust-analyzer
            pkgs.nixfmt
            pkgs.skopeo
          ];
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);

      # buildRustPackage runs `cargo test` in its check phase.
      checks = forAllSystems (
        pkgs:
        let
          pin-oci = self.packages.${pkgs.stdenv.hostPlatform.system}.pin-oci;
        in
        {
          inherit pin-oci;

          clippy = pin-oci.overrideAttrs (old: {
            pname = "pin-oci-clippy";
            nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.clippy ];
            doCheck = false;
            buildPhase = "cargo clippy --offline --all-targets -- -D warnings";
            installPhase = "touch $out";
            postInstall = "";
          });

          rustfmt =
            pkgs.runCommand "pin-oci-rustfmt"
              {
                nativeBuildInputs = [
                  pkgs.rustfmt
                  pkgs.findutils
                ];
              }
              ''
                cd ${pin-oci.src}
                find . -name '*.rs' -print0 | xargs -0 rustfmt --check --edition 2021
                touch $out
              '';

          nixfmt =
            pkgs.runCommand "pin-oci-nixfmt"
              {
                nativeBuildInputs = [
                  pkgs.nixfmt
                  pkgs.findutils
                ];
                src = pkgs.lib.fileset.toSource {
                  root = ./.;
                  fileset = pkgs.lib.fileset.fileFilter (file: file.hasExt "nix") ./.;
                };
              }
              ''
                cd $src
                find . -name '*.nix' -print0 | xargs -0 nixfmt --check
                touch $out
              '';

          module-tests = import ./tests/nix {
            inherit pkgs nixpkgs;
            module = self.nixosModules.default;
          };
          binary-cache = import ./tests/nix/binary-cache.nix {
            inherit pkgs;
            flake = import ./flake.nix;
          };
          overlay = import ./tests/nix/overlay.nix {
            inherit pkgs nixpkgs;
            overlay = self.overlays.default;
            package = pin-oci;
          };
          infra-scripts = import ./tests/nix/infra-scripts.nix {
            inherit pkgs nixpkgs;
            module = self.nixosModules.default;
          };
          # NixOS VM tests: need KVM, kept out of the fast CI job (see .github/workflows/ci.yml).
          vm-infra = import ./tests/nix/vm-infra.nix {
            inherit pkgs;
            module = self.nixosModules.default;
          };
          vm-projects = import ./tests/nix/vm-projects.nix {
            inherit pkgs;
            module = self.nixosModules.default;
          };
        }
      );
    };
}
