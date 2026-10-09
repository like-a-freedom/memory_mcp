# A1 scorer RED evidence

- Base revision: `355dc92206e341d02a61a6066cd7ec5e49f21ce0`.
- Command: `rtk cargo test -p eval-harness --locked --test eval_retrieval_quality`.
- Exit code: 101. The test target compiled and all five tests failed on assertions against the intentionally incomplete scorer stub.
- Observed failures: duplicate test returned precision `0.0` instead of `1.0`; answerable-empty returned `Some(0.0)` instead of `None`; invalid overlapping gold was accepted; prohibited/unknown/budget case returned precision `0.0` instead of `1/3`; declared order returned `None` instead of `Some(false)`.
- This is RED for the new pure scorer contract, not evidence that the old production retrieval service exhibited these exact metric values.
