{
  pkgs,
  nixpkgs,
  module,
}:

# Helpers shared by the offline Nix tests.
let
  inherit (pkgs) lib;
  system = pkgs.stdenv.hostPlatform.system;

  lockOk = ./fixtures/ok.lock;
  lockWrongArch = ./fixtures/wrong-arch.lock;
  lockNoArch = ./fixtures/no-arch.lock;
  lockAbsent = ./fixtures/absent.lock; # intentionally does not exist

  evalOn =
    sys: modules:
    (nixpkgs.lib.nixosSystem {
      system = sys;
      modules = [
        module
        {
          system.stateVersion = "26.05";
          boot.loader.grub.devices = [ "nodev" ];
          fileSystems."/" = {
            device = "none";
            fsType = "tmpfs";
          };
          virtualisation.oci-containers.backend = "podman";
        }
      ]
      ++ modules;
    }).config;
  eval = evalOn system;

  with' = lockFile: containers: {
    virtualisation.pin-oci = {
      enable = true;
      inherit lockFile containers;
    };
  };

  # Messages of passing assertions may be invalid on purpose: only read the failing ones.
  failed = config: lib.filter (a: !a.assertion) config.assertions;
  failedByPinOci = config: map (a: lib.hasInfix "[pin-oci]" a.message) (failed config);
  fails = x: !(builtins.tryEval (builtins.deepSeq x x)).success;

  web = {
    web = {
      image = "caddy:2-alpine";
      ports = [ "80:80" ];
      environment.FOO = "bar";
      volumes = [ "/srv:/srv:ro" ];
    };
  };
in
{
  inherit
    lib
    system
    evalOn
    eval
    with'
    failed
    failedByPinOci
    fails
    web
    lockOk
    lockWrongArch
    lockNoArch
    lockAbsent
    ;
}
