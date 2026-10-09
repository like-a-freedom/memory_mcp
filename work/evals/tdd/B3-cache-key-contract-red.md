# B3 cache-key contract RED

Command: `rtk cargo test -p memory_mcp --locked --lib memory::context_cache::tests -- --nocapture`

Exit: 101. The new tests could not compile because `CacheCutoff` and `CacheView::new_with_cutoff` did not exist. These tests specify exact explicit historical cutoff identity, exact historical window identity, and current/historical namespace separation. The separate embedded integration RED above confirms the underlying observable cache defect.

RED log SHA-256: `d9cf80eba408f46c29ccc0b505a8636a5d9061ace6190a621b7b4a3e3bf97356`. Source hash at this RED point was not captured; it is intentionally not inferred from the later tree.
