# Run CI on cargo only

No Python, no Node, no browser runner in the pipeline.

## Decision

Every check CI runs is a `cargo` command. The tooling that lived under
`scripts/ci` is removed, and the two properties it actually protected move into
the workspace:

| Was | Is now |
|---|---|
| `scripts/ci/package.py` | `cargo run -p xtask -- package` |
| `test_ui_bundle_pin.py`, plus the `test`/`grep` lines in the `Dockerfile` | `cargo run -p xtask -- check-ui-bundle` |
| `assert_embedded_ui.py` against a live container | `crates/memory-mcp/tests/ui_assets.rs`, plus the `docker` job's own walk of the served document |
| `audit_undeclared_sources.py` | removed; see [ADR-0063](0063-retire-the-undeclared-source-audit.md) |
| `audit_doc_claims.py`, `test_doc_claims.py` | removed with the doc-claim citation check |
| `local_admin_image.py`, `local_admin_browser.mjs` (1676 lines) | removed; nothing ran them |

`release.yml` is unchanged: it consumes `dist/*`, which `xtask package` still
produces in the same shape, with the same `.sha256` sidecars and the same
standalone CLI download.

## Why

**Almost none of it ran.** Of twelve files totalling 3516 lines, four were
invoked by the pipeline. `local_admin_image.py` and `local_admin_browser.mjs` —
48% of the directory — were referenced by no workflow at all. The only thing CI
executed from them was `test_local_admin_image.py`, 146 lines asserting that two
scenario lists, one in Python and one in JavaScript, agreed with each other. A
suite no job runs protects nothing from regression, and its size read as
coverage in the evidence tables of `docs/operations/LOCAL_ADMIN.md` while
providing none.

**It failed for the wrong reasons, repeatedly.** Two consecutive pushes failed
on a step that had nothing to do with the tree it inspected: a document-claim
audit that exited because `docs/superpowers/plans` was empty, and an
undeclared-source audit that read a build directory cargo was not writing to.
Both are the signature of tooling coupled to its environment rather than to the
code.

**A second language in the pipeline is a second toolchain to keep working.**
The Python here needed no dependency of its own, but it still had to stay
parseable, lintable and runnable on three platforms, and its failure mode is a
traceback rather than a compiler diagnostic.

## Why not cargo-dist

`xtask package` does seven steps and is exercised by the `native` job on five
platforms. `cargo-dist` covers the same problem with its own configuration
format, plugins and CI integration, and replaces ~250 lines of code with a
dependency, a config file and a tool whose behaviour changes between releases.
The cost is not justified at this size.

## Why the Dioxus asset pipeline is unchanged

The question "is there a native way to do this?" was answered before writing
anything. `asset!` is the framework's own mechanism, and it is not applicable
here: it records an asset's path and options in a linker section and leaves
`dx build` to append the bytes to the executable afterwards, which requires the
asset to be referenced from `rsx!` or pinned with `#[used]`. This deployment
compiles `crates/ui` to WebAssembly and serves those bytes from an HTTP server,
so the bytes have to be inside *that* binary. Only `include_bytes!` can do
that, which is what `crates/memory-mcp/build.rs` already used. Moving to
`asset!` would mean a second binary writing into the first to reach the same
result.

## What is given up

Recorded rather than smoothed over, in `docs/operations/LOCAL_ADMIN.md`:

- **Real-browser interaction.** A click path through the console's own DOM,
  browser console errors and TLS-terminated navigation were only ever exercised
  by the removed harness. The console's *served* properties are still asserted —
  the document, the SPA fallback, a refused path, the CSP with
  `'wasm-unsafe-eval'`, and the same walk against the shipped image — but
  nothing now drives it in a browser.
- **Startup OIDC discovery against a stub provider.** `auth_upgrade.rs` pins
  that every discovery error maps to `ConfigInvalid` and that test composition
  never reaches the network, but it performs no discovery round trip, so an
  unreachable issuer failing a real boot is no longer observed.
- **Doc-claim citation checking.** A `file.rs:NNN` citation written from memory
  instead of measured is no longer caught.

## Consequences

- A contributor needs a Rust toolchain and nothing else.
- `docs/operations/CI.md` describes a pipeline whose steps are all cargo.
