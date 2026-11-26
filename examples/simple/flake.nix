{
  description = "One container, pinned";

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
      nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
        system = "x86_64-linux";
        modules = [
          pin-oci.nixosModules.default
          ./containers.nix
          host
        ];
      };
    };
}
