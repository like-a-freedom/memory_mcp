# Phase 1 extension — uniformly restored upstream image metadata

Reviewed before any Qwen native retrieval predictions or branch-code inspection. Exact input hashes, checks, bindings and group corrections are in `phase-1-image-metadata-extension.json`.

The extension preserves all 35 cases and all 976 source rows. Exactly 133 case-scoped turns, representing 71 distinct source turns, now contain metadata. Every appended `blip_caption`, `query`, and `img_url` value matches the pinned upstream raw record; every original speaker/text prefix is unchanged. Metadata is explicitly labeled as dataset annotation, not a user assertion. This is a uniform source extension, with no evidence of answer-selected inclusion.

The embedded case sources equal the source bundle; its SHA-256 matches both the corpus and preparation metadata. Fact `(source key, type, content)` multisets exactly match the supplied actual preparation inventory. All raw episode binding content matches its source. Existing required keys exist and are allowed. The lineage and binding-change lists match the actual corpus.

The 14 changed fact bindings that inherited allowed status remain useful for their queries. Six additional exact bindings should be allowed: dog-context D1:5 and D2:5 in `locomo-conv-44-000`, and Japanese-mansion planned-stay D1:9 in `locomo-conv-50-001`, each with its corresponding raw episode. The attached JSON gives exact keys and source-only reasons. All other newly appended image material remains subject to the existing query-specific labels; a dog in an unrelated image is not proof of pet ownership.

Two required-group corrections are needed:

- `locomo-conv-42-002`: D1:18 now contains both the relative viewing time and title-identifying metadata. Keep the separate time/title groups, but allow D1:18 or raw:D1:18 to satisfy the title group as well. Forcing D1:16 separately asks for redundant evidence. D1:16 remains useful supplementary evidence.
- `locomo-conv-50-001`: add a required group for D2:5 or raw:D2:5 if complete upstream item identity is claimed. D2:1 does not name Ferrari 488 GTB. D2:5 can also satisfy the car-acquisition group because it says the speaker finally got the pictured car. Mansion support still comes from D1:3. Keep the acquisition-under-specified annotation: the mansion's purchase is not proved by these passages.

The title and Ferrari/Japan referents are now available as labeled upstream image metadata. They are not independently verified image contents or user assertions. Approximate date wording and ambiguous mansion acquisition remain limitations. No cases were dropped and no numerical floor change is proposed.

Whole-branch/code/prediction review remains pending the parent's final package.
