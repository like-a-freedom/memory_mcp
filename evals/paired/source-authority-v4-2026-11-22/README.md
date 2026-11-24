# Source-authority deterministic replay v4

This replay evaluates four fixed public-MCP cases: literal path recall, exact
literal URL recall, an ordinary-fact positive control, and an unknown-reference
abstention control. V4 uses fixed past `t_ref` values. It corrects v3, whose
path and URL events were future-dated relative to ordinary recall; the v3
2/4 result is not evidence that both reference types were unretrievable.

## Identity and method

- Profile: `evals/profiles/memory_decisions_source_authority_v4.json`
- Profile SHA-256: `f864d14d29a25f73785fbabb148f6ee5213a238677e156914a4cd4ca7502ddc8`
- Corpus SHA-256: `f79ccf052b959f443b1d2213f3b258ce674149acae0ebb0fb615aec8ecd229b5`
- Configuration SHA-256: `17ff6b862de83dce474a4e94ab75db5c32b7965b4b8e8265002c338df04de892`
- Input-schema SHA-256: `7b8c60a7d6bdc5d50a71b04e5d2d3dc514516dd69b46627f5b0481cd41c20e61`

Each arm was captured twice with fresh stores. The two baseline replay indexes
match; the two final-candidate indexes match. Every capture has four valid
outcomes and zero invalid outcomes. The baseline and candidate used the same
corpus, configuration, schema and deterministic runtime settings. The
baseline binaries omit the source-authority retrieval correction; the final
candidate binary is the exact build recorded below.

## Results

| Case | Baseline, both repeats | Final candidate, both repeats |
|---|---|---|
| Literal path preservation | Pass | Pass |
| Literal URL preservation | Fail: `incorrect_disposition`, `missing_evidence` | Pass |
| Ordinary fact control | Pass | Pass |
| Unknown-reference abstention | Pass | Pass |
| Primary decision/evidence | 3/4 | 4/4 |

The final-candidate binary SHA-256 is
`9e1191c0f7b10dbe058c0070bdad9ef98adb97c345126de092678460463f4a47` and its
product revision is `2026-11-22-source-authority-v4-final`. Candidate reports
are `final-candidate-a.json` and `final-candidate-b.json`; their replay index
SHA-256 values are `367542f55845f93532a8c1dd0ccd3e90c8562d0c174ab08fed27e904dde767f1`
and `9451b97f3a65490621ddf88c507f740cc4a63b97f69197ddf3caa6b2f95e74b7`.
Both were accepted by `memory-eval compare-decision-replays` as identical.

After source-authority log and validation-response hardening, the final build
was replayed again on two fresh stores to check that those changes did not
regress the v4 contract. Build SHA-256:
`cf47902bccddf5f622b2d46257b1b40fe168139d04502d06e34f2daf2286b505`; product
revision: `2026-11-22-source-authority-v4-final-log-redaction`. Both captures
passed 4/4 primary decisions, 4/4 evidence checks and 4/4 independent probes.
`memory-eval compare-decision-replays` accepted the pair. The raw index files
are not byte-identical: captured receipts contain runtime `t_ingested` and
snapshot `known_at` timestamps. Their SHA-256 values are
`50998c7c12faa0b07f140e6d9f2dd68781a4b28acc9c6dac8208102e0c0f19da` and
`df3882d6c739a1593bcd809238d089da8f45ddc6eec963d626faa1abc27f549f`.
The replay comparison establishes matching deterministic scored outcomes, not
byte-for-byte receipt reproducibility or a new memory-quality uplift.

The latest checkout, including the final entity-name/alias log-redaction
policy, was rebuilt and replayed on two fresh stores. Product binary SHA-256:
`a06d3151ce1e854692e343cfcdbf829cfa26e46f283ca19fc37ae9ff2ff35273`;
product revision: `2026-11-22-source-authority-v4-final-redaction-policy`.
Both repeats passed 4/4 primary decisions, evidence checks and independent
probes; the temporal diagnostic remains 3/4 decisions, 4/4 evidence and 3/4
probes because the unknown-reference control is unobservable at that stage.
The comparator accepted the pair. Index SHA-256 values are
`ed81094084136fde256b894a7bb7cb8fc874fc2b72d94d7bb8d20a04f17d83c5` and
`2308bde5c27420695df4ebeff40a8c39891d1e9ddeacfeb092baa82e683753a3`;
raw receipts differ in runtime timestamps. This final-build check establishes
no regression in the four scored cases, not additional memory-quality gain.

The repeated baseline binary SHA-256 is
`07cb541e7b21aa8a3c03bbc8c13e6ea2e45c728bd33504ed1c7ee983afd12206` and its
revision is `2026-11-22-source-authority-v4-baseline-a`. Baseline A/C indexes
also compare as identical. Baseline B is retained as an exploratory artifact
but is excluded from this paired baseline because it used a different
`product_revision` and was correctly rejected by the replay identity gate.

## Interpretation and limits

This synthetic four-case result shows a repeatable improvement on the exact
URL-reference recall case, with the other three cases unchanged. It does not
establish general memory quality, user or agent convenience, migration
non-inferiority, statistical significance, or security certification. The
separate temporal diagnostic remains 3/4 decisions and 4/4 evidence. See the
[follow-up design](../../../docs/superpowers/specs/2026-11-24-agent-memory-follow-up-design.md)
and [source-authority design](../../../docs/superpowers/specs/2026-11-09-ingestion-source-authority-design.md)
for remaining acceptance gates.
