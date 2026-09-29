# The compiler's dependency graph defines the source tree

> **Superseded by [ADR-0063](0063-retire-the-undeclared-source-audit.md).** The
> analysis below stands — the 137 files were real, and rustc structurally
> cannot report them. The audit it produced has since been removed: it read
> cargo's artifact layout, and on a runner that sets `CARGO_BUILD_TARGET` it
> inspected nothing and reported "no compiled sources were found" — a confident
> failure about a tree it never looked at. This record is kept because the
> reasoning that produced both the retired manifest and the retired audit is
> worth more than the machinery either of them became.

A `.rs` file is part of the build only if some `mod` declaration, `#[path]`
attribute, or `include!` reaches it. A file nothing reaches is invisible to
the entire toolchain: rustc never reads it, so it cannot emit a `dead_code`
warning, a clippy lint, or a compile error; no test executes it; and a search
of the tree still returns it as though it mattered.

That is not a hypothetical. The bounded-context reorganisation (ADR-0058) moved
code into the owning contexts and re-pointed the `mod` declarations at the new
locations, but did not delete the files it had moved past. It left **137 files
/ 54,502 lines — 29.6% of `crates/memory-mcp/src`** on disk that no target
compiled, under any feature combination. The build was green the entire time,
and the compiler was right: from its point of view the tree was correct. Only
the file count disagreed. Several of the pairs were byte-identical.

## Decision

The fact of which files are compiled is derived from **rustc's own dependency
information**, not from a hand-maintained list.

`scripts/ci/audit_undeclared_sources.py` compiles the crate over a feature
matrix wide enough to cover every `mod`-gating feature, reads the `.rs` paths
rustc recorded in `target/debug/deps/*.d`, and fails if any `.rs` under the
crate's `src` never appears. It fails in a second case as well: if the matrix
itself does not compile, or if no dependency files were produced. A guard that
passes because it checked nothing is worse than no guard.

## Why not a manifest

The tree previously carried `docs/architecture/ddd-migration-manifest.csv`: 361
rows, one disposition per source file, kept in step with the tree by hand. It
was retired with the migration record in `6bf227a`, on the grounds that its
remaining rows described an architecture that no longer needed describing.

That reasoning was sound about the manifest's *contents* and wrong about its
*function*. A per-file manifest is the only artefact that can state something
rustc cannot: not merely which files are compiled, but which compiled file is
supposed to own what. The moment the migration stopped, the manifest stopped
being maintained — and with it went the only record that 137 files on disk
should not have been there. Two artifacts in step is a liability; one artifact
in step with the compiler, plus module privacy, is not.

The cost this decision accepts: the audit reports *that* a file is undeclared,
not *who should own its contents*. Judging ownership still needs a reviewer.
What it guarantees is that the question is asked on every build instead of
being answered once and then drifting.

## Relation to the guard retired in `f310a90`

`tests/module_boundaries.rs` policed a different failure: a bounded context
importing across a seam. It was removed because the migration it policed was
done and it had nothing left to catch.

The two guards are disjoint. That one asked *is this import allowed?*; this one
asks *is this file part of the build at all?* A file can pass the first by
being undeclared, and can fail the second while being perfectly well-placed.
ADR-0058's revision, which rests the architecture on Rust's module privacy, is
therefore unaffected: privacy constrains what compiled code may reach, and
neither guard substitutes for it.

## Consequences

- Adding a `.rs` file without a declaration fails CI instead of hiding.
- The feature matrix in the script must stay a superset of every feature that
  gates a `mod` declaration. Features that gate only function bodies, statics
  or dependency crates (`metal`, `accelerate`, `mimalloc`) are deliberately
  excluded, because they cannot introduce an unseen file and including them
  would only make the check slower to reach a slower platform-specific build.
- The audit costs one `cargo check --all-targets` pass, which CI already pays
  for the jobs adjacent to it.
- Files that were undeclared and are now deleted do not need a deprecation
  path: nothing external could reach them, which the audit is what proves.
