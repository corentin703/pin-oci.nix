# Examples

Each directory is a flake with a NixOS configuration. The part that concerns pin-oci is in the
`*.nix` file next to `flake.nix`, so you can copy it into your own configuration.

| Example | Shows |
| --- | --- |
| [`simple/`](simple) | One container. Also the `nix flake init` template. |
| [`app-with-database/`](app-with-database) | A project: an app and its PostgreSQL database, with a volume, start order and a restart target. |
| [`multi-platform/`](multi-platform) | An x86_64 server and an aarch64 NAS sharing one lockfile. |

None of them ships a `pin-oci.lock`: a digest is data from the registry, so you generate it.
From the example directory:

```console
$ git init && git add .          # a flake only sees files git tracks
$ nix run github:corentin703/pin-oci.nix -- lock --flake .
$ git add pin-oci.lock
$ nix flake check                # or nixos-rebuild --flake .#<host>
```

Before `pin-oci lock`, evaluation fails with the command to run: that is the guarantee, not a
bug. The Nix tests evaluate these modules against a fixture lockfile
(`tests/nix/examples.nix`).
