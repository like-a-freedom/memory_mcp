# Independent entity-admission development audit

The 16 multilingual source labels were frozen before predictions in `evals/corpora/entity-admission-multilingual-v1/registration.json`. One before capture and one registered after capture use public ingestion/extraction on isolated real stores. `raw` retains actual entities, facts and episode-linked extraction links. There are no invalid or omitted cases. Labels are single-author reviewed synthetic development labels, not blind human confirmation.

| Measure | Before v2 | After v3 |
| --- | ---: | ---: |
| Exact closed cases | 10/16 | 12/16 |
| Identity TP / FP / FN | 6 / 4 / 3 | 6 / 2 / 3 |
| Micro precision | 0.600 | 0.750 |
| Micro recall | 0.667 | 0.667 |
| Micro F1 | 0.632 | 0.706 |
| Expected source-linked identities | 6/9 | 6/9 |
| All predicted identities have matching episode links | 10/10 | 8/8 |

The actual repair admits neither French article-plus-acronym measure labels nor Russian measure predicates as identities, when followed by an Anno-typed numeric value. Independent GCV controls fail before and pass after. A genuine French actor occurrence of the same spelling survives; a manually resolved identity survives automatic filtering. The grammar is finite, not a universal multilingual or acronym classifier. Extractor fingerprint advances to `anno|identity-admission=v3`; old projections are not deleted or silently reformed.

By class after: currencies 2/2, units 1/2, metrics 3/3, digit names 0/2, organizations 2/2, people 3/3, project 0/1, explicit ambiguous identity 1/1. Language totals must be read with class composition; these tiny counts are not language-level population estimates.

Remaining observed findings: German generic “Das Paket” becomes a false identity; `3M` and `Studio 54` without the old reporting predicate are missed. `Project Cedar` is returned as that full name, while the frozen label expects `Cedar`: this exact-name failure remains counted, but identity correctness is disputed until a canonical-name policy/source review decides it. The label is not changed after seeing the prediction. Adding brand/verb/determiner whitelists to raise these four scores would not establish general extraction quality.

Linking has two distinct meanings: IDs correctly link to their actual source (8/8), yet false identities also receive links. Expected-identity coverage is only 6/9. These figures must not be collapsed into “100% linking quality”. Historical automatic projections retain their prior fingerprint and source audit trail; changing future formation is not a migration or a claim that old stored noise disappeared.

Reproduction: `ENTITY_ADMISSION_CAPTURE=/absolute/new.json cargo test -p eval-harness --test extraction_suite_contract observe_registered_multilingual_admission --locked -- --ignored`. Files are create-new; failed calls would remain scheduled invalid rows. Run the ordinary extraction suite separately for its existing regression gate. Source-only native inventory refresh and binding comparison are recorded alongside the active retrieval corpus, not by substituting the new fingerprint into old results.
