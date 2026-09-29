#!/usr/bin/env python3
"""Fail if any source file is on disk but never compiled.

A `.rs` file under `crates/memory-mcp/src` is part of the build only if some
`mod` declaration, `#[path]`, or `include!`/`include_str!` reaches it. A file
that no declaration reaches is invisible to the entire toolchain: rustc never
reads it, so it cannot produce a `dead_code` warning, a clippy lint, or a
compile error; no test executes it; and a grep of the tree still returns it.

That is how the bounded-context migration left 137 files / 54,502 lines behind
— 29.6% of the crate — after moving their contents into the owning contexts.
The compiler agreed the tree was correct, because from its point of view it
was. Only the file count disagreed.

The check is derived from rustc's own dependency information rather than from a
hand-maintained manifest, so it cannot drift from what is actually built:

  1. Run `cargo check --all-targets` over a feature matrix wide enough to
     compile every `mod` in the crate.
  2. Read every `.rs` path rustc recorded in `target/debug/deps/*.d`.
  3. Any `.rs` on disk under the crate that never appears is undeclared.

Two failure modes are treated as failures, because a guard that passes
because it checked nothing is worse than no guard:

  * the matrix fails to compile, so the set of "compiled" files is a
    partial snapshot rather than a fact;
  * the set of undeclared files is non-empty.

The feature matrix must be a superset of every `#[cfg(feature = ...)]` that
gates a `mod` declaration. Features that only gate function bodies, statics or
dependency crates (`metal`, `accelerate`, `mimalloc`) cannot introduce an
unseen file and are deliberately left out: including them would only make the
check slower to reach a slower platform-specific build.
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CRATE_SRC = os.path.join("crates", "memory-mcp", "src")
PACKAGE = "memory_mcp"

# Every feature that gates a `mod` or `pub mod` declaration in the crate.
#
# `streamable-http` is the coarse SaaS switch and implies `control-plane`,
# `ui` and `prometheus`; listing them keeps the matrix readable and states the
# internal names explicitly rather than relying on that implication to hold.
# `test-fixtures` and `eval-support` expose `cfg`-gated modules that integration
# tests and the eval harness compile; without them the auditor would report
# live files as undeclared.
FEATURES = (
    "streamable-http",
    "control-plane",
    "ui",
    "prometheus",
    "mcp-apps",
    "fs-watch",
    "eval-support",
    "test-fixtures",
)

SOURCE = re.compile(r"\S+\.rs")


def run_matrix():
    """Compile the crate over the feature matrix. Returns (ok, output)."""
    command = [
        "cargo",
        "check",
        "-p",
        PACKAGE,
        "--all-targets",
        "--features",
        ",".join(FEATURES),
    ]
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    return result.returncode == 0, result.stdout + result.stderr


def deps_dir():
    """The directory rustc wrote this run's dep-info files into.

    Cargo nests the profile under the target triple whenever
    `CARGO_BUILD_TARGET` is set, which every CI job does — the audit then
    read an empty `target/debug/deps` and reported "no compiled sources",
    a failure that had nothing to do with the tree it inspects. Without
    that variable Cargo omits the triple entirely, which is the case this
    function has to keep working for too.
    """
    target = os.environ.get("CARGO_BUILD_TARGET", "").strip()
    if not target:
        return os.path.join(ROOT, "target", "debug", "deps")
    return os.path.join(ROOT, "target", target, "debug", "deps")


def compiled_sources():
    """Every source path rustc recorded reading, as paths relative to ROOT."""
    deps = deps_dir()
    if not os.path.isdir(deps):
        return set()
    found = set()
    for name in sorted(os.listdir(deps)):
        if not name.endswith(".d"):
            continue
        with open(os.path.join(deps, name), encoding="utf-8", errors="ignore") as handle:
            text = handle.read()
        # A dep-info file is a target line, a blank line, then the sources it
        # read. Splitting on the blank line keeps us off the target path, which
        # lives in target/ and is not a source file.
        for block in text.split("\n\n")[1:]:
            for path in SOURCE.findall(block):
                if path.startswith(CRATE_SRC.replace(os.sep, "/") + "/"):
                    found.add(os.path.normpath(path))
    return found


def on_disk_sources():
    """Every .rs under the crate's src, as paths relative to ROOT."""
    base = os.path.join(ROOT, CRATE_SRC)
    found = set()
    for directory, _dirs, names in os.walk(base):
        for name in names:
            if name.endswith(".rs"):
                full = os.path.join(directory, name)
                found.add(os.path.normpath(os.path.relpath(full, ROOT)))
    return found


def main():
    ok, output = run_matrix()
    if not ok:
        sys.stderr.write(
            "the feature matrix failed to compile, so the undeclared-source "
            "audit cannot report a result:\n"
        )
        sys.stderr.write(output)
        return 1

    compiled = compiled_sources()
    if not compiled:
        sys.stderr.write(
            "no compiled sources were found in target/debug/deps. The audit "
            "would pass without checking anything; run the matrix above and "
            "confirm rustc produced dependency files.\n"
        )
        return 1

    undeclared = sorted(on_disk_sources() - compiled)
    if undeclared:
        sys.stderr.write(
            f"{len(undeclared)} source file(s) are on disk but never compiled. "
            "No `mod` declaration, `#[path]`, or `include!` reaches them, so "
            "rustc cannot warn about them and no test runs them.\n\n"
        )
        for path in undeclared:
            sys.stderr.write(f"  {path}\n")
        sys.stderr.write(
            "\nEither delete the file, or add the declaration that makes it "
            "part of the build. A copy of logic that already lives with its "
            "owner belongs to neither.\n"
        )
        return 1

    print(
        f"every source file is compiled "
        f"({len(on_disk_sources())} files, features: {','.join(FEATURES)})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
