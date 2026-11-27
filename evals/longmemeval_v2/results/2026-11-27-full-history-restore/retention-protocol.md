# Closed-store retention protocol

Registered before the build finishes. Never move or copy a store with an active
writer. First require the single build process to exit successfully and its
`build-coverage.json` to report the saved complete build. Preserve the original
30-minute resource failure separately from the operational rescue deadline.

Hash every regular file in the closed saved store, recording relative names,
sizes and SHA-256. Move the directory (same filesystem) to the ignored local
`data/eval-snapshots/full-history-2026-11-27/store` directory. Create a symlink at
the originally registered temporary path so the already frozen capture driver
and its identity do not change. Compare the complete file manifest through the
new stable path and old alias before starting the source audit. This physical
relocation does not recreate, reopen, mutate or retimestamp episodes.

Keep the stable snapshot until user acceptance; it is the source of fresh
disposable clones, never a directly evaluated mutable store. The two audit
clones must return exactly the same 151 source/body digests and episode IDs.
Each of the seven registered reads then receives its own fresh clone. Check
the retained snapshot file manifest after both phases. File hashes prove that
the snapshot is unchanged; episode-level readback separately proves the
registered source inventory. Neither alone is a reader-quality score.

Store execution receipts and the compact manifest here. Store the database
outside tracked evidence. If the temporary alias is removed, recreate it from
the stable catalog, or supply that stable directory to the existing ignored
Rust observer using `MEMORY_FULL_HISTORY_STORE`. If the database is lost, use
the pinned source preparation and source-only build recipe; do not substitute
a surviving partial store or relabel it as the complete history.
