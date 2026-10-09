# B3 historical cache RED

Command: `rtk cargo test -p memory_mcp --locked --features test-fixtures --test embedded_context_cache historical_cutoffs_in_one_bucket -- --nocapture --test-threads=1`

Exit: 101. Both real embedded-service scenarios failed after setup completed. With earlier cutoff first, the later cutoff reused the cached empty result (`left: 0`, `right: 1`). With later cutoff first, the earlier cutoff reused the later result (`before.is_empty()` failed). Both cutoffs were explicitly placed on opposite sides of a future `t_valid` instant inside one 5-minute bucket, while fact ingestion used the normal production service path.

Test source SHA-256 at RED: `8d2bb6f3831775ff64369b20588182a81f86bc0c92f33f016ca4b88aa3194865` (`tests/embedded_context_cache.rs`). RED log SHA-256: `3fde50e68dd59a7d6f6d26995bb69c0773d2418591ca4fba01d3bffe6a369b7f`.

This is a product-behavior RED, not a setup or compilation failure.
