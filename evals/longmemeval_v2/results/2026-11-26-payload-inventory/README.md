# Payload inventory diagnosis — 26 November 2026

Base product: `a74d509b`; production code unchanged. Native Apple Silicon only,
no models or LM Studio. These are **two metadata process attempts**, separate
from the20 retained full-context development attempts. No new quality answers.

The registered full-table attempt observed **427 facts** in8.286ms. Its next
projection of character lengths and SHA256 for every content/index key exceeded
the45s external bound (process45.111s, SIGKILL). That projection includes expensive
per-key SQL functions; this is **not an isolated product/storage latency**.
No full-table size or duplication statistics were obtained. Completed count
metadata survived because each query boundary was immediately flushed.

A separately registered adaptive diagnostic read only the first stored fact.
It completed (whole process14.268s; projection407.035ms), observing:

- fact `fact:00995bc1b210ed06c3a4ea7e`, source `episode:88aae65596a1cd0c9703e401`;
- content66,026 Unicode scalar characters;
-2,193 index keys,37,471 total characters, maximum215;
-2,193 distinct index-key SHA256 values: no repeated raw keys in this row.

This is not a representative sample, a query candidate pool, UTF8 byte counts,
or proof of normalization overlap. Raw-key deduplication alone has no established
benefit for this observed row. The results support a concrete input shape for a
resource control; they do not yet prove a retrieval repair.

Two real-engine calibrations verify exact exported fields, character counts,
known Unicode SHA256 values and an episode without index keys. The first setup
expected an absent field to be omitted; the actual driver returnsnull. The failed
literal expectation is retained, then corrected. This is characterization and
setup correction, **not a production defect or claimed runtime product RED**.

The unsuccessful full-table mapper diagnostic has been removed from active
source. Both historical source versions are retained as `candidate.patch`, with
pre-capture hashes in registrations. The useful ignored first-row diagnostic and
two calibrated controls remain in `tests/full_history_evidence.rs`. Only metadata
is exported; original source content is not printed by these diagnostics.

Next: collect actual query-pool count/size/unique metadata at completed pipeline
boundaries, with incremental persistence and low measurement overhead. Avoid
whole-table per-key SQL mapping for that collection. Then test computing a
membership-only index-key vocabulary once per record (preserving duplicate query
hits, index-only matches, wrapped driver values, anchor weights and exact order).
Measure actual candidate-pool repetition before adding broader caches or changing
hydration. Do not lower source limits, remove terms or infer30s acceptance.

Full-history acceptance remains0/2; archived answer accuracy remains2/7, not a
new result. Old C1 stays stopped; C2 is not admitted. Original snapshot hashes
match before/after both attempts. Final checks are recorded in `verification.json`.
