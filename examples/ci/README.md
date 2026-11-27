# CI examples

| File | Platform | Does |
| --- | --- | --- |
| [`github-check.yml`](github-check.yml) | GitHub Actions | `pin-oci lock check` on every pull request |
| [`forgejo-check.yml`](forgejo-check.yml) | Forgejo Actions | same |
| [`github-update.yml`](github-update.yml) | GitHub Actions | weekly `pin-oci lock update`, then a pull request |
| [`forgejo-update.yml`](forgejo-update.yml) | Forgejo Actions | same, through the Forgejo API |

Copy a file to your repository's workflows directory (the path is in its first line) and adjust
`runs-on`. The CLI version in `nix run github:corentin703/pin-oci.nix/v0.4.0` is a release tag:
move it when you upgrade.

- **`lock check`** needs no network access to a registry and writes nothing: it fails (exit 1)
  when a declared image has no digest for the host platform. Run it on every pull request.
- **`lock update`** contacts the registries, so private ones need credentials in the job.
  It rewrites only `pin-oci.lock`; the workflow turns that into a pull request that your own
  checks (build, `lock check`, a deploy dry run) validate before anyone merges.
- A flake has to see the lockfile: the workflows run in a git checkout, so it is tracked.
