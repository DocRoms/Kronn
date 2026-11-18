# Backend crates

The backend is a Cargo workspace of five crates stacked in layers. One rustc
process compiles one crate and its peak memory follows the crate's size: as a
single crate of about 875 000 lines, the instrumented test build needed 17 to
21 GB and was killed on the 16 GB CI runner (KT-1854). No crate may grow back
to that size: two CI gates enforce it (see "Limits enforced in CI").

## Layout

| Crate | Root file | Depends on |
|---|---|---|
| `kronn-l0` | `backend/src/layer0.rs` | external crates only |
| `kronn-l1` | `backend/src/layer1.rs` | `kronn-l0` |
| `kronn-l2` | `backend/src/layer2.rs` | `kronn-l0`, `kronn-l1` |
| `kronn-l3` | `backend/src/layer3.rs` | `kronn-l0` to `kronn-l2` |
| `kronn` (server binary, integration tests) | `backend/src/lib.rs` | all four |

`backend/Cargo.toml` is the workspace root and the `kronn` package; the layer
crates have a manifest each under `backend/crates/l<k>/` whose library path
points back to `backend/src/layer<k>.rs`. Source files did not move: every
crate's modules live in `backend/src/<top>/`, and a file belongs to the crate
whose root declares its module.

Every crate exposes the same paths. A layer root re-exports the layer below
and adds its own modules:

```rust
pub mod core {
    pub use kronn_l0::core::*;
    pub mod audit_validation;   // a module owned by this layer
}
```

so `crate::core::config` and `crate::core::audit_validation` are written the
same way in every crate that can see them. The lowest crate that owns part of
a top-level module declares it in `backend/src/<top>/mod.rs`; the layers above
declare theirs in their root file.

A crate sees its own modules and those of the layers below, never above:
Cargo refuses a dependency cycle, so a module that starts using something from
a higher layer no longer compiles until it is moved up or the call is inverted.

## Adding or moving a module

- A new module goes in the lowest crate that holds everything it uses, as long
  as that crate stays under the size limit. Declare it in that crate's root
  (`pub mod <top> { … pub mod <name>; }`), or in `<top>/mod.rs` for layer 0 of
  that top-level module.
- To move a module, move its `pub mod` line from one root to the other. Paths
  in the rest of the code do not change.
- Items used from another crate must be `pub`: `pub(crate)` stops at the crate.
- `impl` blocks of a type stay in the crate that defines the type.

## Calling up: hooks

A lower crate cannot name a higher one. When it needs code that lives above
(a route handler, a workflow tool), the lower crate declares a hook type and
the higher crate registers its implementation with `inventory`:

```rust
// lower crate — backend/src/core/uplink.rs
pub struct PageResources(pub fn(&Connection, &str) -> Result<Option<Vec<LivePageResource>>>);
inventory::collect!(PageResources);

// higher crate
inventory::submit! { crate::core::uplink::PageResources(list_resources) }

// caller, in the lower crate
let hook = crate::core::uplink::required::<PageResources>()?;
```

Hooks that take the application state are in `core::uplink_state`; the
database boot steps use the same mechanism (`db::BootStep`). The server lists
every hook in `kronn::missing_uplinks()` and refuses to start when one has no
provider; a test fails when a declared hook is missing from that list.

## Tests

- **Own tests** stay beside the code, in the crate of the module.
- **Test helpers used by other crates** are behind the `test-support` feature
  of the layer crates (`#[cfg(any(test, feature = "test-support"))]`). The
  crates above enable it through their dev-dependencies only; the server is
  linked without it, and CI checks that (`cargo tree`, quality job).
- **Tests that reach into a higher layer** are compiled by the crate of that
  layer. The test code is in a `*_up.rs` file next to the module; a wrapper in
  `backend/src/uptests/` gives it the scope of the module it tests, and the
  higher crate's root lists the wrapper in `mod uptests`. Such a test is named
  `uptests::<top>__<module>__<tests>::…`.
- **Behaviour that differs in a unit-test process** (short timers, refusal to
  write the real config) asks `core::test_mode::on()` when the process runs,
  not `cfg(test)` alone: a test binary of a higher crate links the lower
  crates as ordinary libraries, and the integration binary and the server
  must keep the real behaviour.
- **Repository paths** in code and tests come from `crate::BACKEND_DIR` (the
  `backend/` folder in every crate), not from `CARGO_MANIFEST_DIR`, which is
  `backend/crates/l<k>` in a layer crate.

Every crate is a default member of the workspace, so a bare `cargo clippy` or
`cargo test --lib <filter>` in `backend/` covers the five of them; CI and the
Makefile still say `--workspace`. The server is built with `-p kronn`, which
resolves the features of the server and nothing else. Commands: `cargo nextest run
--workspace`, `cargo clippy --workspace --all-targets`, and `make typegen`
exports the TypeScript bindings of every crate into `backend/bindings/`.

## Limits enforced in CI

| Gate | What it refuses | When |
|---|---|---|
| `backend/scripts/ci/crate_size.py` | A crate over 200 000 lines (its root and every module file reached from it, tests included) | Quality job, before any compilation |
| `backend/scripts/ci/rustc_memory.py` | A crate whose compilation peaked over 8 GB of resident memory | After the instrumented build, from the peaks `scripts/ci/rustc_one_at_a_time.sh` records |

When the size gate fails, move modules to another crate (the lowest one that
can hold them) or add a layer crate: a new `backend/crates/l<k>/Cargo.toml`
and its root file. Raising the limit is not the fix: the memory gate measures
what the limit is there to prevent.
