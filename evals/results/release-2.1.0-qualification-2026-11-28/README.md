# v2.1.0 publication qualification and recovery

Artifact source: `6740421e41c1f7d4716442c6b875f5a7c92f0737`; immutable tag `v2.1.0`.
Qualified hosted CI: [46058402331](https://github.com/like-a-freedom/memory_mcp/actions/runs/46058402331), successful.
The initial publisher [46072830377](https://github.com/like-a-freedom/memory_mcp/actions/runs/46072830377) failed before mutations: its read-only metadata job could not inspect the draft. The new master orchestrator retains every existing job permission and moves draft inspection to the already write-authorized publisher.

`checks.json` and `eval-checks.json` retain local release gates. `qualified-release.json` and `qualified-image.json` describe candidate bytes, not proof of public availability. Sixteen public files are expected. Failed controls and the failed dispatch remain preserved; subsequent recovery receipts must use separate files.

Final local checks: nine executable ref guards, six executable draft guards, 22 publisher integration tests, unchanged-permission and explicit-tag policy checks. Policy checks are not production scenario coverage. The single review finding (ambiguous checkout ref) was fixed with an explicit tag ref. No new artifact build or inference was used for this recovery.

Replay the input guards from repository root using Python with PyYAML installed:

```sh
python3 evals/results/release-2.1.0-qualification-2026-11-28/check_ref_controls.py
python3 evals/results/release-2.1.0-qualification-2026-11-28/check_draft_controls.py
cargo test -p xtask --test release_publication --locked
```

The orchestrator commit and artifact-source commit are separate identities. Publication is complete: [Release46074161545](https://github.com/like-a-freedom/memory_mcp/actions/runs/46074161545) succeeded. [Independent live verification](recovery-publication-verified.json) downloaded all16 public files and matched every SHA-256, verified the annotated tag, qualified bundle provenance, OCI version/revision, identical version/SHA/latest digests, and GitHub latest. The build job was skipped; quality work continued during publication.
