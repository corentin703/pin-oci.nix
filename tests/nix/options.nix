{
  lib,
  system,
  eval,
  with',
  failed,
  lockOk,
  ...
}:

# Every oci-containers option must keep working through pin-oci: a container defined with
# pin-oci must produce exactly what the same container defined directly would.
let
  pinned = "docker.io/library/caddy@sha256:${lib.concatStrings (lib.replicate 64 (if system == "aarch64-linux" then "c" else "a"))}";

  # Every option of the container submodule that makes sense without login/image files.
  rich = {
    cmd = [
      "--flag"
      "x y"
    ];
    entrypoint = "/bin/entry";
    environment = {
      A = "1";
      B = "two words";
    };
    environmentFiles = [ ./fixtures/ok.lock ];
    ports = [
      "80:80"
      "127.0.0.1:8443:443/tcp"
    ];
    user = "1000:1000";
    volumes = [ "/srv:/srv:ro" ];
    labels.l = "v";
    workdir = "/w";
    hostname = "h";
    log-driver = "journald";
    pull = "always";
    autoStart = false;
    privileged = true;
    autoRemoveOnStop = false;
    capabilities = {
      NET_ADMIN = true;
      SYS_ADMIN = false;
    };
    devices = [ "/dev/fuse" ];
    networks = [ "host" ];
    extraOptions = [
      "--init"
      "--memory=64m"
    ];
    preRunExtraOptions = [ "--log-level=debug" ];
    serviceName = "custom-web";
    podman = {
      user = "root";
      sdnotify = "container";
    };
  };

  viaPinOci = eval [ (with' lockOk { web = rich // { image = "caddy:2-alpine"; }; }) ];
  direct = eval [
    { virtualisation.oci-containers.containers.web = rich // { image = pinned; }; }
  ];

  container = c: lib.filterAttrs (n: _: n != "image") c.virtualisation.oci-containers.containers.web;
  unit = c: c.systemd.services.custom-web;
in
{
  tests = {
    testEveryContainerOptionBehavesLikeOciContainers = {
      expr = container viaPinOci == container direct;
      expected = true;
    };
    testTheGeneratedUnitIsIdenticalToTheDirectOne = {
      expr = {
        script = (unit viaPinOci).script == (unit direct).script;
        pinnedImage = lib.hasInfix pinned (unit viaPinOci).script;
        serviceConfig = (unit viaPinOci).serviceConfig == (unit direct).serviceConfig;
        wantedBy = (unit viaPinOci).wantedBy == (unit direct).wantedBy;
        after = (unit viaPinOci).after == (unit direct).after;
        requires = (unit viaPinOci).requires == (unit direct).requires;
      };
      expected = {
        script = true;
        pinnedImage = true;
        serviceConfig = true;
        wantedBy = true;
        after = true;
        requires = true;
      };
    };
    testNoOptionIsLostOrAdded = {
      expr =
        let
          pinOci = builtins.attrNames (removeAttrs viaPinOci.virtualisation.pin-oci.containers.web [ "_module" ]);
          oci = builtins.attrNames direct.virtualisation.oci-containers.containers.web;
        in
        pinOci == oci;
      expected = true;
    };
    testDocumentedOptionsAreAllCovered = {
      # `rich` must set every option of the submodule that has no mandatory side input,
      # otherwise a new nixpkgs option would silently escape this comparison.
      expr = lib.subtractLists (builtins.attrNames rich ++ [ "image" "imageFile" "imageStream" "login" "dependsOn" ]) (
        builtins.attrNames direct.virtualisation.oci-containers.containers.web
      );
      expected = [ ];
    };
  };
}
