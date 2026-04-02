# Lifecycle Background Jobs

**Status:** ✅ Implemented (2026-03-26)
**Implementation:** `src/service/lifecycle/` module

## Overview

The Memory MCP server includes optional background workers that maintain memory hygiene:

1. **Confidence Decay Worker** - marks stale facts as invalid
2. **Episode Archival Worker** - archives old episodes without active facts

Both workers are **disabled by default** and must be explicitly enabled via environment variables.

## Implemented Features

The following lifecycle jobs are now fully implemented:

- **confidence decay refresh**
  - Periodically recomputes decayed confidence for all active facts
  - Marks facts with decayed confidence < threshold as invalid
  - **Configuration:** `LIFECYCLE_DECAY_INTERVAL_SECS` (default: 1 hour)
  - **Threshold:** `LIFECYCLE_DECAY_THRESHOLD` (default: 0.3)
  - **Heat-aware:** Skips recently-accessed facts (`access_count > 0` and `last_accessed` within half-life)

- **episode archival**
  - Archives episodes older than threshold without active facts
  - Preserves data but excludes from default queries
  - **Configuration:** `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` (default: 24 hours)
  - **Age threshold:** `LIFECYCLE_ARCHIVAL_AGE_DAYS` (default: 90 days)
  - **Heat-aware:** Skips episodes with facts accessed within `LIFECYCLE_ARCHIVAL_AGE_DAYS / 2`

- **embedding backfill** (REMOVED from scope)
  - ~~Populate missing embeddings after a real provider is enabled~~
  - ~~Reindex vector fields after dimension or provider changes~~
  - **Status:** Superseded by `SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — embeddings removed from runtime

## Heat-Aware Lifecycle (Adaptive Memory)

As of 2026-03-27, lifecycle workers use access heat signals to protect active memories:

### Access Heat Tracking

- **`access_count`** (int, default 0): Incremented on every fact access
  - `+1` for retrieval via `assemble_context`
  - `+3` for citation via `explain` (stronger signal)
- **`last_accessed`** (datetime, nullable): Updated to `time::now()` on access

Implementation uses SurrealDB atomic updates:
```sql
UPDATE type::thing('fact', $id) SET access_count += $boost, last_accessed = time::now()
```

### Decay Worker Heat Check

Before invalidating a fact due to low decayed confidence, the decay worker checks:

```rust
let is_hot = access_count > 0
    && last_accessed.is_some_and(|la| (now - la).num_days() as f64 <= half_life_days);

if decayed < threshold && !is_hot {
    // invalidate fact
}
```

This prevents frequently-accessed facts from being invalidated purely due to age.

### Archival Worker Heat Check

The archival worker skips episodes that have any facts accessed recently:

```sql
SELECT fact_id FROM fact 
WHERE source_episode = $episode_id 
  AND last_accessed IS NOT NONE 
  AND last_accessed >= type::datetime($hot_cutoff) 
LIMIT 1
```

If any hot facts exist, the episode is preserved regardless of age.

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `LIFECYCLE_ENABLED` | false | Enable background workers |
| `LIFECYCLE_DECAY_INTERVAL_SECS` | 3600 | Decay job interval |
| `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | 86400 | Archival job interval |
| `LIFECYCLE_DECAY_THRESHOLD` | 0.3 | Confidence threshold |
| `LIFECYCLE_ARCHIVAL_AGE_DAYS` | 90 | Episode age threshold |

## Implementation Status

| Component | Status | Location |
|-----------|--------|----------|
| Decay worker | ✅ Implemented | `src/service/lifecycle/decay.rs` |
| Archival worker | ✅ Implemented | `src/service/lifecycle/archival.rs` |
| Configuration | ✅ Implemented | `src/config.rs::LifecycleConfig` |
| Service integration | ✅ Implemented | `src/service/core.rs::new_from_env()` |
| Documentation | ✅ Implemented | `.env.example`, `README.md` |

## How It Works

### Decay Worker

Runs every `LIFECYCLE_DECAY_INTERVAL_SECS` seconds:

1. Fetches all active facts from database
2. Computes decayed confidence: `base * exp(-λ * days)`
   - λ = 0.693 / 365 (half-life 1 year)
3. Marks facts with decayed < threshold as invalid:
   - Sets `t_invalid` = now
   - Sets `t_invalid_ingested` = now

### Archival Worker

Runs every `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` seconds:

1. Fetches all episodes from database
2. Filters by age > `LIFECYCLE_ARCHIVAL_AGE_DAYS`
3. Checks if episode has active facts
4. Archives episodes without active facts:
   - Sets `status` = "archived"
   - Sets `archived_at` = now

## Enabling Lifecycle Workers

### Development

```bash
export LIFECYCLE_ENABLED=true
export LIFECYCLE_DECAY_INTERVAL_SECS=300  # 5 minutes for testing
export LIFECYCLE_ARCHIVAL_INTERVAL_SECS=600  # 10 minutes for testing
cargo run
```

### Production

```bash
export LIFECYCLE_ENABLED=true
# Defaults are reasonable for most deployments
cargo run --release
```

## Monitoring

Workers log structured events:

```
[2026-03-26T10:00:00Z] INFO op=lifecycle.workers.started decay_interval=3600 archival_interval=86400
[2026-03-26T10:00:00Z] INFO op=lifecycle.decay.start interval_secs=3600 threshold=0.3
[2026-03-26T10:00:05Z] INFO op=lifecycle.decay.complete facts_invalidated=12
[2026-03-26T10:00:00Z] INFO op=lifecycle.archival.start interval_secs=86400 age_days=90
[2026-03-26T10:00:10Z] INFO op=lifecycle.archival.complete episodes_archived=3
```

Errors are logged with `op=lifecycle.*.error` and include the error message.

## Troubleshooting

### Worker not starting

1. Check `LIFECYCLE_ENABLED=true` is set
2. Verify service startup logs for `lifecycle.workers.started`
3. Ensure intervals are positive integers

### Too many facts being invalidated

1. Increase `LIFECYCLE_DECAY_THRESHOLD` (e.g., 0.5)
2. Increase base confidence for important facts during ingestion
3. Review decay formula if business requirements changed

### Episodes being archived too aggressively

1. Increase `LIFECYCLE_ARCHIVAL_AGE_DAYS` (e.g., 180)
2. Ensure important facts are not being invalidated prematurely
3. Check if facts are being properly linked to episodes

## Performance Considerations

- Workers run asynchronously and do not block request handling
- Each pass scans entire tables (O(n) complexity)
- For large deployments (>100k facts), consider:
  - Increasing intervals to reduce frequency
  - Adding database indexes on `t_valid`, `t_invalid`
  - Implementing batched/paginated scans in future iterations

## Future Enhancements

Potential improvements for later iterations:

- **Batched scanning** - process facts in chunks to reduce memory pressure
- **Selective decay** - different decay rates per fact type (promises vs metrics)
- **Archive storage tier** - move archived episodes to cold storage
- **Reactivation workflow** - manual or automatic un-archival if new evidence appears
