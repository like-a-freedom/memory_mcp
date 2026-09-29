# Retire the undeclared-source audit

The audit this supersedes caught a real defect once and has no replacement.

## Status

Supersedes [ADR-0061](0061-compiler-graph-defines-the-source-tree.md).

## What ADR-0061 decided, and what it caught

ADR-0061 derives "which `.rs` files are part of the build" from rustc's own
dependency information rather than from a hand-maintained manifest, and fails
the build when a file on disk never appears in any `.d` file. The defect it
existed for was real: the bounded-context reorganisation moved code to its new
owners and re-pointed the `mod` declarations without deleting the files it had
moved past, leaving **137 files / 54,502 lines — 29.6% of the crate** compiled
by nothing, with the build green throughout.

That reasoning was sound. The guard that grew from it is removed here anyway,
because of what it cost.

## Why it goes

**It fails for reasons unrelated to the tree it inspects.** Every CI job sets
`CARGO_BUILD_TARGET`, which makes cargo nest the profile under the target
triple: the audit read `target/debug/deps` while rustc wrote
`target/<triple>/debug/deps`. It therefore found no dep-info files and reported

    no compiled sources were found in target/debug/deps. The audit would pass
    without checking anything; run the matrix above and confirm rustc produced
    dependency files.

The message tells the reader that the crate's sources are suspect. The run had
inspected none of them. That is precisely the failure ADR-0061 called "worse
than a false positive" — a guard that reports nothing because it checked
nothing — except in the form of a failure, so it costs every push a bisect
across a defect that does not exist. It did so twice before the cause was
found, and the mode is reproducible from the environment alone rather than
from anything about the code.

**It reads rustc's build directory, which is not an interface.** The audit's
only input is the layout cargo happens to use for artifacts on a given machine.
That layout moved once already — a directory rename to
`target/<llvm-cov-target>/debug/deps` is visible in this repository — and each
such change presents as "your sources are wrong".

**Nothing consumed its output.** It reported file paths; every real instance was
resolved by a person reading the path. The 137 files were deleted by hand.

## What is given up

The gap ADR-0061 closed is real and reopening it is a deliberate choice: **no
check now reports a `.rs` file that no `mod` declaration reaches.** Rust
tooling structurally cannot: such a file is never read by rustc, so it cannot
produce a `dead_code` warning, a lint, or a compile error.

What still holds:

- A file that is declared but broken fails the build, as any compiled file does.
- A file whose `mod` path is wrong fails to compile.
- The bounded contexts remain reachable only through `mod` declarations, and
  module privacy still constrains what compiled code may reach.

What no longer holds: an orphan file can sit on disk indefinitely and nobody is
told. If that cost is judged too high later, the right form is a check that does
not depend on cargo's artifact layout — `cargo metadata` resolves the workspace
but not `mod` reachability, so a check would have to walk the module tree
itself and reconcile it against the filesystem. That is a real program, not a
script, and it should be written when someone wants it rather than inherited as
a fragile one.

## Relation to the removal of the rest of `scripts/ci`

This is one of several decisions taken together: the browser-driven image
harness and the Python tooling under `scripts/ci` are removed, packaging moves
to `crates/xtask`, and CI runs nothing outside cargo. See
[ADR-0064](0064-run-ci-on-cargo-only.md).

## Consequences

- An undeclared `.rs` file is no longer reported by any automated check.
- `ADR-0061` stays in the record as the analysis that produced both the retired
  manifest and the retired audit; neither is deleted, because the reasoning that
  they were right is worth more than the machinery.
