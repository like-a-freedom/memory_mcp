#!/usr/bin/env python3
"""Audit checkable claims in the evidence docs against the tree.

Three classes, because those are the ones a reviewer keeps catching:
  1. `file.rs:NNN` citations that point at the wrong line or a missing file
  2. `snake_case` identifiers cited as tests/functions that do not exist
  3. bare numeric claims ("N items", "N tests", "N rows")

Nothing here edits anything. It reports so the claims can be corrected
before a reviewer has to.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# ADRs that describe the architecture as it currently stands, and so are held to
# the tree. Everything below 0058 is a historical record of an earlier design:
# it names APIs that have since been removed (`select_record`, `service_context`)
# and is never rewritten, so auditing it would produce only false failures. A
# record of what was must not be held to the tree as it is.
#
# ADR-0062 joins this set when the control-plane persistence split lands; until
# then it does not exist, and listing a missing number is harmless.
LIVE_ADRS = frozenset({"0058", "0060", "0061", "0062"})


def discover_docs():
    """Every markdown file under the evidence and planning trees.

    Discovered rather than listed, so a document added later is checked
    instead of silently escaping — and so the self-test's probe file is
    seen like any other claim-bearing document.

    This migration's own spec is included explicitly; earlier specs are
    historical records and are excluded along with the ADRs.

    Every configured tree is asserted to exist. A tree that silently
    disappears — as `docs/architecture` and `docs/superpowers/plans` both did
    when the migration record was retired — narrows the audit to a fraction of
    the documents while it still reports success, which is the one outcome
    worse than a false positive.
    """
    docs = []
    for tree in (
        "docs/superpowers/plans",
        "docs/operations",
        "docs/adr",
    ):
        if not os.path.isdir(os.path.join(ROOT, tree)):
            sys.exit(
                f"{tree} is configured for the doc-claim audit but does not "
                f"exist. Restore it, or remove it from the tree list in "
                f"discover_docs() so the audit stops claiming to cover it."
            )
        for base, _dirs, names in os.walk(os.path.join(ROOT, tree)):
            # Historical ADRs name APIs since removed and are never rewritten,
            # so they are excluded. These three describe the architecture as it
            # stands, so they are audited like everything else: 0058 is the
            # bounded-context reorganisation, 0060 closed the last context to
            # `service` edges, and 0061 made the compiler graph the source-tree
            # contract. Historical ADRs under `docs/architecture/decisions/` were
            # excluded for the same reason before that tree was retired in
            # `6bf227a`.
            if base.endswith(os.path.join("docs", "adr")):
                names[:] = [n for n in names if n[:4] in LIVE_ADRS]
            for name in sorted(names):
                if name.endswith(".md"):
                    docs.append(os.path.relpath(os.path.join(base, name), ROOT))
    docs.append("docs/superpowers/specs/2026-09-23-ddd-modular-monolith.md")
    docs.append("CONTEXT.md")
    # `release-gate-4ea7c39.md` freezes a count as of its own revision.
    # Historical, like the ADRs.
    docs = [d for d in docs if not d.endswith("release-gate-4ea7c39.md")]
    return sorted(docs)


DOCS = discover_docs()

CITE = re.compile(r"`([A-Za-z0-9_/.-]+\.rs):(-?\d+)(?:-(-?\d+))?`")
IDENT = re.compile(r"`([a-z][a-z0-9_]{4,})`")

problems = []


def read(path):
    with open(os.path.join(ROOT, path), encoding="utf-8") as handle:
        return handle.read().splitlines()


def source_files():
    """Every .rs file under crates/, keyed by its basename and by suffix."""
    found = {}
    for base, _dirs, names in os.walk(os.path.join(ROOT, "crates")):
        for name in names:
            if name.endswith(".rs"):
                full = os.path.join(base, name)
                rel = os.path.relpath(full, ROOT)
                found.setdefault(name, []).append(rel)
    return found


def resolve(cited, sources):
    """A citation names a file by suffix; find the unique match.

    Several files share a basename (`logging.rs` exists three times), so
    an unqualified citation of one is ambiguous however correct its line
    number is. Callers should cite enough path to be unique.
    """
    matches = [p for p in sum(sources.values(), []) if p.endswith("/" + cited) or p.endswith(cited)]
    if len(matches) == 1:
        return matches[0]
    return None


def check_line(path, line_no, cited, doc, where, line_end=None):
    full = os.path.join(ROOT, path)
    with open(full, encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    if line_no < 1 or (line_end is not None and line_end < 1):
        problems.append(
            f"{doc}:{where} cites {cited}:{line_no} with a non-positive line number"
        )
        return
    hi = line_end if line_end is not None else line_no
    if hi < line_no:
        problems.append(f"{doc}:{where} cites {cited}:{line_no}-{line_end} with a reversed range")
        return
    # Only the span's endpoints must point at content. Interior blank
    # or closing-brace lines are normal in a `:N-M` citation.
    for at in dict.fromkeys([line_no, hi]):
        if at > len(lines):
            problems.append(
                f"{doc}:{where} cites {cited}:{at} but that file has {len(lines)} lines"
            )
            continue
        body = lines[at - 1].strip()
        if not body or body in ("{", "}", "};", "})"):
            problems.append(
                f"{doc}:{where} cites {cited}:{at} which is `{body or 'blank'}` — the line has no content to point at"
            )


def check_counts(problems):
    """Class 3: bare numeric claims that the tree can settle.

    Each entry is (what it measures, how to measure it, where the claim
    is written as `<n> <unit>`). A claim is only checked once its pattern
    is found in the docs; a claim that disappears entirely is reported, so
    the checker cannot pass by never matching anything. Prose counts with
    no unambiguous referent ("203 paths at capture") are deliberately not
    listed — guessing at those would trade false confidence for noise.
    """
    root = ROOT

    def read(rel):
        with open(os.path.join(root, rel), encoding="utf-8") as handle:
            return handle.read()

    def rust_tests(rel):
        return len(re.findall(r"^\s*#\[(?:tokio::)?test\]", read(rel), re.M))

    def py_tests(rel):
        return len(re.findall(r"^\s*def test_", read(rel), re.M))

    # (claim pattern with one `<n>` capture, measured value, unit label)
    #
    # This list is empty, and deliberately so. Every per-file count claim the
    # auditor existed for was written in the DDD migration record, which has
    # been retired: the manifest that listed the per-file dispositions is
    # gone, and the boundary checker that read it was removed. None of those
    # counts appear in any surviving document, so there is no numeric claim
    # left here that the tree can settle.
    #
    # A claim whose subject no longer exists is not a stale number to
    # correct — silently re-measuring it would keep the audit asserting a
    # fiction. Rewriting such prose is a job for whoever rewrites the prose.
    #
    # The citation and identifier checks above are unaffected: they are not
    # count-dependent, and they are what catch a doc pointing at a line that
    # no longer exists. A future count claim added to a live document should
    # be registered here, keeping the per-pattern staleness check below, so a
    # claim cannot quietly stop being verified.
    claims: list[tuple[str, int, str]] = []

    words = {"one": 1, "two": 2, "three": 3, "four": 4, "five": 5,
             "six": 6, "seven": 7, "eight": 8, "nine": 9, "ten": 10,
             "eleven": 11, "twelve": 12}

    def as_int(token):
        return int(token) if token.isdigit() else words.get(token.lower())

    hits = {pattern: 0 for pattern, _m, _u in claims}
    for doc in DOCS:
        for index, line in enumerate(read(doc).splitlines(), start=1):
            for pattern, measured, unit in claims:
                for match in re.finditer(pattern, line):
                    hits[pattern] += 1
                    claimed = as_int(match.group(1))
                    if claimed != measured:
                        problems.append(
                            f"{doc}:{index} claims {match.group(1)} {unit}, tree has {measured}"
                        )
    # Per-pattern, not global: one pattern going stale used to hide
    # behind the others still matching, so a claim could stop being
    # checked without anyone noticing.
    for pattern, _measured, unit in claims:
        if hits[pattern] == 0:
            problems.append(
                f"check_counts: the pattern for {unit} matched nothing — "
                "the claim is gone or its wording drifted"
            )

    # The release-gate table that held the one remaining count claim was part
    # of the DDD migration record and was retired with it, so there is no
    # gate table left to check and no `python_suite` helper to measure it.
    #
    # If that gate is ever restored, register its Python-suite total in
    # `claims` above rather than re-adding a bespoke check here: the claims
    # list carries the per-pattern staleness guard that keeps a check from
    # silently going dead, and a one-off block would not.


def main():
    sources = source_files()
    all_source_text = {}
    for paths in sources.values():
        for p in paths:
            with open(os.path.join(ROOT, p), encoding="utf-8") as handle:
                all_source_text[p] = handle.read()

    # Identifiers that legitimately appear as prose words rather than code.
    stop = {
        "target", "result", "source", "forbidden", "optional", "assert",
        "unwrap", "expect", "async", "return", "struct", "public", "static",
        "import", "format", "collect", "extend", "insert", "remove", "clone",
        "default", "equals", "value", "error", "errors", "null", "true",
        "false", "whole", "whole_file", "forbidden_layer",
    }

    for doc in DOCS:
        for index, line in enumerate(read(doc), start=1):
            for match in CITE.finditer(line):
                cited, start = match.group(1), int(match.group(2))
                path = resolve(cited, sources)
                if path is None:
                    matches = [
                        p for p in sum(sources.values(), [])
                        if p.endswith("/" + cited) or p.endswith(cited)
                    ]
                    if matches:
                        problems.append(
                            f"{doc}:{index} cites `{cited}:{start}` ambiguously — "
                            f"{len(matches)} files match: {', '.join(sorted(matches))}"
                        )
                    else:
                        problems.append(f"{doc}:{index} cites `{cited}:{start}` but no such file exists")
                    continue
                check_line(
                    path,
                    start,
                    cited,
                    doc,
                    index,
                    int(match.group(3)) if match.group(3) else None,
                )

    # Identifier existence. A cited name may be a function or test in the
    # Rust tree, a Python test in `scripts/ci`, a cargo target name, or a
    # test/bench file stem. All four are legitimate citations, so all four
    # are searched.
    corpi = dict(all_source_text)
    for base, dirs, names in os.walk(os.path.join(ROOT, "scripts")):
        # Vendored JS and Python bytecode caches are not source. The
        # caches mattered: `__pycache__/test_doc_claims.*.pyc` holds the
        # probe's planted symbol, so the corpus "found" the name in the
        # compiled form of the test asserting it is missing.
        dirs[:] = [d for d in dirs if d not in ("node_modules", "__pycache__")]
        names[:] = [
            n for n in names
            if n not in ("test_doc_claims.py", "audit_doc_claims.py") and not n.endswith(".pyc")
        ]
        for name in names:
            full = os.path.join(base, name)
            rel = os.path.relpath(full, ROOT)
            with open(full, encoding="utf-8", errors="replace") as handle:
                corpi[rel] = handle.read()
    for base, _dirs, names in os.walk(os.path.join(ROOT, "crates")):
        for name in names:
            if name in ("Cargo.toml",):
                full = os.path.join(base, name)
                with open(full, encoding="utf-8") as handle:
                    corpi[os.path.relpath(full, ROOT)] = handle.read()
    stems = {
        os.path.splitext(os.path.basename(p))[0]
        for p in sum(sources.values(), [])
    }

    for doc in DOCS:
        if doc.startswith("docs/operations/"):
            # Operator runbooks name CLI commands, tables and target
            # architectures in backticks. Symbol existence is not a
            # meaningful check there; citations and counts still are.
            continue
        for index, line in enumerate(read(doc), start=1):
            for match in IDENT.finditer(line):
                name = match.group(1)
                if name in stop or name.endswith(".rs") or "/" in name:
                    continue
                # A revision hash is a legitimate citation, not an identifier.
                if re.fullmatch(r"[0-9a-f]{7,40}", name):
                    continue
                if name in stems:
                    continue
                if not any(name in text for text in corpi.values()):
                    problems.append(f"{doc}:{index} cites `{name}` which appears nowhere in the tree")

    check_counts(problems)

    if problems:
        print(f"{len(problems)} claim problems:\n")
        for problem in problems:
            print(f"  - {problem}")
        sys.exit(1)
    print("all cited files, line numbers and identifiers resolve")


if __name__ == "__main__":
    main()
