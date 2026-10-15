{ lib }:

{
  # Canonical form of an image reference (BIBLE §2). The only place where references are parsed.
  #   redis:7                   -> docker.io/library/redis:7
  #   localhost:5000/foo        -> localhost:5000/foo:latest
  #   redis@sha256:...          -> pinned = true (passed through untouched by the module)
  parseRef =
    ref:
    let
      pinned = lib.hasInfix "@" ref;
      withoutDigest = builtins.head (lib.splitString "@" ref);
      parts = lib.splitString "/" withoutDigest;
      first = builtins.head parts;
      # Docker's rule: the first segment is a registry if it looks like a host.
      hasRegistry =
        builtins.length parts > 1
        && (lib.hasInfix "." first || lib.hasInfix ":" first || first == "localhost");
      registry = if hasRegistry then first else "docker.io";
      path = if hasRegistry then lib.concatStringsSep "/" (builtins.tail parts) else withoutDigest;
      lastSegment = lib.last (lib.splitString "/" path);
      hasTag = lib.hasInfix ":" lastSegment;
      tag = if hasTag then lib.last (lib.splitString ":" lastSegment) else "latest";
      untagged = if hasTag then lib.removeSuffix ":${tag}" path else path;
      repository =
        if registry == "docker.io" && !(lib.hasInfix "/" untagged) then "library/${untagged}" else untagged;
    in
    if tag == "" || untagged == "" || lib.hasSuffix "/" untagged then
      throw "[pin-oci] invalid image reference '${ref}'"
    else
      {
        inherit
          pinned
          registry
          repository
          tag
          ;
        key = "${registry}/${repository}:${tag}";
      };
}
