# AGENT PROTOCOL — pin-oci.nix

> This file is the **single source of the agent protocol** for this repository.
> `CLAUDE.md` is a symlink to it. Short, command-verifiable instructions, zero ambiguity.

## STARTUP — Minimal context (in this order, nothing more)
1. **Read** this file.
2. **Check the repository state:** `git log --oneline -10` + `ls`. Never assume the layout described in `BIBLE.md` §7 already exists.
3. **Read** only the section of `BIBLE.md` relevant to the task (`grep -n '^## ' BIBLE.md`, then read the useful line range). Do not load the whole file for a targeted task.
4. **Acknowledge:** `PROTOCOL LOADED. Mode: LOCKED.`
5. **Wait** for the user's instruction.

**Document hierarchy** (upstream wins on divergence): `BIBLE.md` > code > commit messages.
Commit messages are never proof of progress — only verified code is.

## LOCKED MODE (default)
Execution only. **Zero initiative.** One task = one identifiable piece of work from the BIBLE (a section, or a sub-point explicitly requested).

The agent does NOT, unless explicitly asked:
- refactor, rename, reformat or "clean up" code outside the task scope;
- add features, options, flags, abstractions or error handling that were not requested;
- add, upgrade or remove dependencies;
- touch files unrelated to the task;
- make a design decision the BIBLE or the user has not settled;
- push, open pull requests, tag, or release.

**Flow:**
```
INPUT → GOAL (1 line)
      → STEPS (max 5, atomic)
      → PLAN
      → `VALIDATION: OK` from the user required
      → EXECUTE (atomic)
      → ERROR? STOP + diagnosis
      → SUCCESS? full DoD (below) + closure report
```

## ARCHITECT MODE
**Activation:** `Architect Mode` | `Critique [X]` | `Propose [X]`
- Critical analysis + 2–3 alternative plans, with explicit risks and gains.
- No code is written. Waits for validation, then returns to LOCKED automatically.

## ABSOLUTE RULE — EVERY FEATURE MUST BE TESTABLE AND TESTED
**No feature is accepted unless it is designed to be testable and ships with its tests.**

- Testability is a design constraint: logic is separated from I/O (network, filesystem, `nix` invocation) so it can be exercised without them. If a feature cannot be tested as designed, STOP and ask — do not ship it untested.
- Tests are written **in the same task** as the feature, never deferred.
- A fixed bug gets an immediate **regression test** that would have caught it.
- Guarantees stated in the BIBLE (fail-fast assertion, purity at evaluation time, deterministic lockfile output, idempotent systemd units, etc.) each have a test that **tries to falsify them**, not only a happy-path test.

| Level | Rust CLI (`pin-oci`) | Nix modules | Required for |
|-------|----------------------|-------------|--------------|
| **Unit** | `#[test]`, zero I/O (reference parsing, manifest → lock mapping, serialization) | `nix eval` assertions on pure functions / option resolution | Any logic: ≥ 2 cases (happy path + edge/error) |
| **Integration** | `tests/`, registry mocked at the HTTP boundary; `nix` invocation behind a trait/stub | module evaluation against fixture lockfiles (present / missing / wrong arch → assertion message) | Any component crossing a boundary |
| **End-to-end** | CLI binary run against a fixture config | NixOS VM test (`nixosTest`) or equivalent for generated systemd units / containers | Any user-visible command or option |

Tests must run offline and deterministically; no test may depend on a live registry.

## DEFINITION OF DONE — Mandatory at EVERY task closure
A task is done only if **all** the following proofs exist and are provided in the closure report:

| # | Gate | Required proof |
|---|------|----------------|
| 1 | Builds | `cargo check` and/or `nix flake check` / `nix eval`, per touched scope |
| 2 | Tests | output of the test command for the touched scope (unit + integration + e2e as applicable) |
| 3 | **Real wiring** | `grep` showing the new code is **reached from the production path** (CLI subcommand, imported module), not only from tests. Code that exists but is not wired = task NOT done |
| 4 | **No placeholder** | no stub, `todo!()`, `unimplemented!()` or hard-coded value returned in place of a real result |
| 5 | **Purity preserved** | no network access added to Nix evaluation or build |
| 6 | Scope respected | `git diff --stat` touches only files the task required |
| 7 | Commit | the task is **committed** — nothing counts as done while sitting in the working tree |

Never write "complete" / "done" if any BIBLE guarantee touched by the change is not actually verified in the delivered code.

## ABSOLUTE RULES
| Rule | Action |
|------|--------|
| Source of truth | `BIBLE.md` > code > assumptions (never commit messages) |
| Ambiguity | STOP + immediate question |
| Refactoring | Only on explicit request |
| Dependencies | Explicit validation required |
| Runtime error | STOP → diagnosis → question |
| Placeholder / stub | FORBIDDEN to present as delivered |
| Tests | Mandatory — see the testing rule above |
| Visibility | Nothing `pub` by default |
| Security | No bypass without justification |
| Execution engine | `virtualisation.oci-containers` stays the only runtime layer — never reimplement it |
| Evaluation purity | No network access during `nix build` / `nixos-rebuild`; resolution lives in the CLI only |
| Fail-fast | Unlocked images must fail at evaluation with the exact command to run |

## TONE
Direct. Dry. Technical. No chatter. Closure report = commands run + key outputs + BIBLE guarantees verified.

## STOP CONDITIONS
| Condition | Action |
|-----------|--------|
| Task incomplete/ambiguous, or the BIBLE does not settle a design point | STOP — clarify with the user before coding any assumption |
| Persistent error | STOP + detailed diagnosis |
| Request would require leaving LOCKED scope | STOP — ask, or propose via Architect Mode |
