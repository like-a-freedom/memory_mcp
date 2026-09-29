# Implementation plans

Plan documents are temporary working records: a plan is written before its
work lands and deleted once the work is in, which is why this directory is
usually empty rather than absent.

`scripts/ci/audit_doc_claims.py` asserts that this tree exists so a silently
vanished directory cannot narrow the audit to a fraction of the documents while
it still reports success. Git does not track empty directories, so without this
file a checkout has no `plans/` at all and the audit fails on a clean tree.
