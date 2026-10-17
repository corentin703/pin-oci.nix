{ pkgs, flake }:

# The flake advertises its binary cache. Pure: reads the `nixConfig` attribute of flake.nix.
let
  inherit (pkgs) lib;
  cfg = flake.nixConfig or { };
  substituters = cfg.extra-substituters or [ ];
  keys = cfg.extra-trusted-public-keys or [ ];

  hostOf = url: lib.removePrefix "https://" url;
  # A key is "<host>-<n>:<base64>" and must belong to one of the declared substituters.
  keyHost = key: lib.head (lib.splitString ":" key);
  keyMatchesHost = key: lib.any (url: lib.hasPrefix "${hostOf url}-" (keyHost key)) substituters;

  tests = {
    testSubstituterDeclared = {
      expr = substituters;
      expected = [ "https://pin-oci.cachix.org" ];
    };
    testAllSubstitutersAreHttps = {
      expr = lib.all (lib.hasPrefix "https://") substituters;
      expected = true;
    };
    testKeyDeclared = {
      expr = keys;
      expected = [ "pin-oci.cachix.org-1:rnJNoJhMLyEkTckqap7U3sTpic1xvACQlpSNqFHTPys=" ];
    };
    # Falsification: a key that does not belong to a declared substituter is rejected.
    testEveryKeyMatchesASubstituter = {
      expr = lib.all keyMatchesHost keys;
      expected = true;
    };
    testMismatchedKeyIsDetected = {
      expr = keyMatchesHost "other.example.org-1:AAAA";
      expected = false;
    };
    # Never replace the default trust: only `extra-*` settings.
    testDoesNotOverrideDefaults = {
      expr = lib.any (n: n == "substituters" || n == "trusted-public-keys") (lib.attrNames cfg);
      expected = false;
    };
  };
  failures = lib.debug.runTests tests;
in
pkgs.runCommand "pin-oci-binary-cache-tests" { } (
  if failures == [ ] then
    ''
      echo "${toString (builtins.length (builtins.attrNames tests))} binary cache tests passed"
      touch $out
    ''
  else
    ''
      echo ${lib.escapeShellArg (builtins.toJSON failures)} >&2
      exit 1
    ''
)
