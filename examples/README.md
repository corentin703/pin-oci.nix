# Examples

Each directory is a flake with a NixOS configuration. The part that concerns pin-oci is in the
`*.nix` file next to `flake.nix`, so you can copy it into your own configuration.

| Example | Shows |
| --- | --- |
| [`simple/`](simple) | One container. Also the `nix flake init` template. |
| [`app-with-database/`](app-with-database) | A project: an app and its PostgreSQL database, with a volume, start order and a restart target. |
| [`multi-platform/`](multi-platform) | An x86_64 server and an aarch64 NAS sharing one lockfile. |
| [`rootless/`](rootless) | A project run by one rootless Podman user, with the users declared by you (`users.nix`). Adapted from a production host. |
| [`rootless-multi/`](rootless-multi) | Several projects, one rootless Podman user each, derived from one table of uids (`oci-lib.nix`) and declared by you (`oci-users.nix`). Adapted from a production host. |

`app-with-database`, `multi-platform`, `rootless` and `rootless-multi` ship the `pin-oci.lock` that `pin-oci lock`
wrote, so you can see what it looks like next to the module that reads it. It is a snapshot: the
digests are those of the day it was generated and they age, so generate your own. `simple` ships
none, because `nix flake init` copies it into your repository. A digest is data from the registry,
so you generate it. From the example directory:

```console
$ git init && git add .          # a flake only sees files git tracks
$ nix run github:corentin703/pin-oci.nix -- lock --flake .
$ git add pin-oci.lock
$ nix flake check                # or nixos-rebuild --flake .#<host>
```

Without a lockfile (`simple`, or an image the lock does not know), evaluation fails with the
command to run: that is the guarantee, not a bug. The Nix tests evaluate these modules against a
fixture lockfile and against the lockfiles they ship (`tests/nix/examples.nix`).
