{ pkgs, module }:

# End-to-end: with `pullImage`, the image comes from the Nix store and the machine never
# reaches a registry (NixOS test VMs have no network). `dockerTools.pullImage` is replaced by a
# locally built archive carrying the same name and tag; what is verified is the wiring the
# module generates (imageFile, image name, load before start), not the fetch itself.
pkgs.testers.runNixOSTest {
  name = "pin-oci-pull-image";

  globalTimeout = 600;

  # The nodes apply an overlay, which a read-only `pkgs` would forbid.
  node.pkgsReadOnly = false;

  defaults =
    { pkgs, ... }:
    {
      imports = [ module ];
      nixpkgs.overlays = [
        (final: prev: {
          dockerTools = prev.dockerTools // {
            pullImage =
              args:
              final.dockerTools.buildImage {
                name = args.finalImageName;
                tag = args.finalImageTag;
                copyToRoot = [ final.busybox ];
                config.Cmd = [
                  "sleep"
                  "100000"
                ];
              };
          };
        })
      ];
      virtualisation.pin-oci = {
        enable = true;
        lockFile = ./fixtures/pull.lock;
        containers = {
          pulled = {
            image = "caddy:2-alpine";
            pullImage = true;
          };
        };
      };
    };

  nodes.podman =
    { ... }:
    {
      virtualisation.oci-containers.backend = "podman";
    };

  nodes.docker =
    { ... }:
    {
      virtualisation.oci-containers.backend = "docker";
    };

  testScript = ''
    start_all()

    for machine, backend in [(podman, "podman"), (docker, "docker")]:
        with subtest(f"{backend}: the container starts from the store image, offline"):
            machine.wait_for_unit("multi-user.target")
            machine.wait_for_unit(f"{backend}-pulled.service")
            machine.fail("ping -c1 -W1 1.1.1.1")

        with subtest(f"{backend}: it runs the locked name and tag, not a digest reference"):
            # The unit is active as soon as `run` is spawned: wait for the container itself.
            machine.wait_until_succeeds(f"{backend} inspect pulled")
            image = machine.succeed(
                f"{backend} inspect pulled --format '{{{{.Config.Image}}}}'"
            ).strip()
            assert image.endswith("caddy:2-alpine"), image
            assert "@sha256" not in image, image
  '';
}
