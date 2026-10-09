# A1 scorer and eval-harness GREEN evidence

- rtk cargo test -p eval-harness --locked: 234 passed, 13 ignored across 12 suites.
- The package includes the scorer contract, retrieval-quality manifest and source-bundle validation, native runtime identity checks, artifact publication, and strict release-gate tests.
- Testing the package validates harness contracts. It does not run a corpus score.
- No retrieval-quality corpus manifest/source bundle or matching native provider/index preparation is present, so no real-model/native acceptance result is claimed.
