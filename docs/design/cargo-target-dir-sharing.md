# Decision — one `CARGO_TARGET_DIR` per worktree, not one shared across them

**Status:** decided (2026-08-22) · **Origin:** KT-373 DoD-12, after the 2026-08-21 disk-full incident

## The question

Seven worktrees held seven Rust `target/` directories, 7.5 to 24.6 GiB each, and
filled a 2 TB volume. Sharing one `CARGO_TARGET_DIR` between them is the obvious
saving. The ticket explicitly refused to adopt it implicitly: parallel builds may
contend on locks and invalidate each other's artefacts. This is the measurement
that decides it.

## What was measured

Two trivial crates, built cold and concurrently, once into a shared target
directory and once into isolated ones. Trivial on purpose: the question is
whether Cargo *serialises*, and that answer does not depend on graph size.

| | build-directory lock | package-cache lock | disk |
|---|---|---|---|
| Shared `CARGO_TARGET_DIR` | **`Blocking waiting for file lock on build directory`** | yes | 2.1 MiB |
| One target per crate | none | yes | 1.0 + 1.0 MiB |

Cargo takes an **exclusive lock on the build directory**. Two concurrent builds
sharing one target directory do not run in parallel; the second waits for the
first. With isolated targets that lock disappears.

**Limit of this measurement, stated plainly:** it proves the *mechanism*, not the
*magnitude*. Trivial crates cannot show how long a real Kronn build would wait
behind another, nor how much two branch checkouts would actually share. Anyone
who needs that number must measure it on the real workspace.

## A side result that explains something we lived through

`Blocking waiting for file lock on package cache` appears in **both** columns.
That cache is `~/.cargo/registry`, global to the machine and shared no matter how
targets are arranged. During this very session one agent reported its build
waiting on another's, while their worktrees had separate targets — this is why.
Isolating targets removes build-directory contention and nothing else.

## Decision

**Keep one target per worktree.** Reclaim space through lifecycle cleanup —
retiring `target/` when its execution is durably finished — rather than by
sharing one directory between agents.

Three reasons, in order of weight:

1. **Sharing serialises exactly what we run in parallel.** Kronn's whole point
   here is several agents working at once. Trading their parallelism for disk is
   the wrong trade when the disk can be reclaimed instead.
2. **The saving is smaller than it looks.** Sharing pays off when builds reuse
   the same dependency artefacts. Worktrees sit on *different branches*: their
   fingerprints diverge as soon as a dependency, feature or profile differs, and
   the shared directory then holds several generations of everything rather than
   one. The main repository's own `target/debug` reached **1 689 865 files** this
   way.
3. **It concentrates the failure.** A shared directory cannot be cleaned without
   coordinating every agent and watcher at once — the 21st needed exactly that,
   and took over 44 minutes of I/O. A per-worktree target is removed in one
   operation, on the authority of one execution's durable state.

Sharing *within* one checkout stays as it is: `.cargo/config.toml` already points
`backend/` and `desktop/src-tauri/` at one target, which mutualises ~200 common
crates for builds that were never going to run concurrently anyway.

## What this commits us to

Isolated targets only pay off if something reclaims them. That is the rest of
KT-373: a guard that refuses provisioning below a configurable floor, a scan that
inventories what could be reclaimed, and a cleanup authorised by durable state —
never by the absence of a visible compiler.
