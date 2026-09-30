# ADR-0065: Reinstate the source-tree and doc-claim guards as cargo tests

- Status: accepted
- Date: 2026-09-30
- Partly reinstates: ADR-0061
- Reverses in part: ADR-0063, ADR-0064
- Related: ADR-0058, ADR-0069

## Context

Two facts were true of this repository and nothing enforced either.

**A `.rs` file is code only if a `mod` declaration reaches it.** ADR-0061
established this and paid for it: the bounded-context reorganisation
(ADR-0058) moved code to its new owners and re-pointed the `mod` declarations,
but did not delete the files it moved past. It left **137 files / 54,502
lines — 29.6% of `crates/memory-mcp/src`** on disk that no target compiled,
under any feature combination, with the build green throughout. rustc
structurally cannot report this: a file it never reads cannot produce a
`dead_code` warning, a lint, or a compile error.

The guard ADR-0061 built was retired by ADR-0063 for a reason that was correct
and is still correct. The script read cargo's artifact layout; every CI job
sets `CARGO_BUILD_TARGET`, which makes cargo nest the profile under the target
triple, so the script looked in `target/debug/deps` while rustc wrote
`target/<triple>/debug/deps`. It found nothing and printed

    no compiled sources were found in target/debug/deps.

A confident failure about a tree it had never inspected, reproducing on any
runner that sets that variable. ADR-0063's closing note named the right
remedy: *a check that does not depend on cargo's artifact layout — a check
would have to walk the module tree itself and reconcile it against the
filesystem.*

Then ADR-0064 required CI to run nothing outside cargo, and that removed the
way to keep such a script honest. The gap was left open.

**A document that cites a file which does not exist misleads.** `docs/`
carried nineteen citations into an empty `docs/superpowers/plans/`, naming
thirteen targets that are not there. ADR-0064 removed the audit that reported
them because it exited early on the empty directory and reported nothing —
the same failure mode as the script above, in the same commit.

## Decision

Both checks come back, as cargo tests.

ADR-0064 requires CI to run nothing outside cargo, and a cargo test is the only
shape that satisfies that. This is not a compromise: a test is a better shape
than a script here anyway, because it runs in the same invocation as everything
else, cannot drift from the build it describes, and cannot be skipped by a
runner that forgot a step.

**`crates/memory-mcp/tests/source_tree_integrity.rs`** walks the `mod`
declarations in the source and asserts every `.rs` file under `src/` is named
by one. It reads the declarations themselves rather than inferring them from a
build directory, so there is no artifact layout to drift and no feature matrix
to keep a superset: the 306 `cfg`-gated `mod` declarations are read whether or
not their feature is enabled, which is strictly more than the retired script
could see. The crate roots Cargo compiles are read out of `Cargo.toml`, so a
new binary target is not misreported as an orphan. A companion test asserts
the crate uses no `#[path]` attribute, which the resolver does not implement —
so the first one added fails loudly instead of quietly.

**`crates/memory-mcp/tests/doc_claims.rs`** asserts that every relative
markdown link resolves, that every ADR's link and backticked `docs/…`
reference resolves, and that each spec's `**Status:**` line is one of three
exact values and — for a spec claiming `Implemented` — names ADRs that exist.
The status table is written by hand, because the question is whether the
document matches the code and the code is the thing under audit.

Both guards were **observed failing** before they were trusted. The
source-tree guard was run against a deliberately undeclared probe file; the
doc-claim guard reported the nineteen dangling citations and the two inverted
spec status lines before any of them were fixed. A guard that has never failed
is a comment.

## Consequences

- Two integration test binaries are added to CI. They are cheap: the
  source-tree guard walks 384 files in about 0.15 s, and the doc-claim guard
  reads markdown.
- The source-tree guard must be taught about `#[path]` the day the crate uses
  one, and about a new module layout the day one appears. The alternative is a
  guard that reports the truth incorrectly, which is worse than no guard.
- The spec status table must be updated deliberately. That is the cost: a new
  spec cannot be added without someone stating what its status is, and an
  existing spec cannot silently drift from the code.
- **ADR-0061, ADR-0063 and ADR-0064 are not deleted, and this ADR is the
  reason a future reviewer does not propose removing these tests a third
  time.** Three accepted decisions are being reversed or qualified here, which
  is precisely the situation where the reasoning has to be written down.
- ADR-0069 was written while implementing the doc-claim check, because the
  status assertion surfaced a fully-implemented feature with no ADR at all. It
  is a separate decision and has its own record.

## Alternatives considered

1. **Fix the Python audit and keep it as a cargo `[[test]]` target.** Rejected
   as a later option, not a wrong one. It would still read cargo's artifact
   layout, and reading the `mod` declarations is both simpler and stricter: it
   sees files behind features this profile does not enable.

2. **Keep a hand-maintained manifest of source files, as ADR-0061's
   predecessor did.** Rejected, and ADR-0061 already made the argument: two
   artefacts kept in step is a liability, and the moment the migration stopped,
   the manifest stopped being maintained.

3. **Rely on `cargo check --all-targets` plus a review checklist.** Rejected.
   This is the status quo ante, and it is what produced 137 orphans with a
   green build: the toolchain cannot see an undeclared file, and a checklist
   is not a check.

4. **A single guard file for both checks.** Rejected. They share nothing but
   the reason to exist, and merging them would make the failure message of one
   depend on the traversal of the other.
