{
  description = "One lockfile for an x86_64 server and an aarch64 NAS";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
    pin-oci = {
      url = "github:corentin703/pin-oci.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, pin-oci, ... }:
    let
      # Replace with your own hardware-configuration.nix. Only here so the example evaluates.
      host = {
        system.stateVersion = "26.05";
        boot.loader.grub.devices = [ "nodev" ];
        fileSystems."/" = {
          device = "none";
          fsType = "tmpfs";
        };
      };
    in
    {
      nixosConfigurations = {
        server = nixpkgs.lib.nixosSystem {
          system = "x86_64-linux";
          modules = [
            pin-oci.nixosModules.default
            ./common.nix
            host
          ];
        };
        nas = nixpkgs.lib.nixosSystem {
          system = "aarch64-linux";
          modules = [
            pin-oci.nixosModules.default
            ./common.nix
            ./nas.nix
            host
          ];
        };
      };
    };
}
