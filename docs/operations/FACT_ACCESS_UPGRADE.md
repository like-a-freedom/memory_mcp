# Fact access storage upgrade and rollback

Migration 057 copies the live access counter and last access time from legacy `fact` rows to `fact_access`. The application then reads and updates those values in the side table. The fact still carries the legacy fields for rollback. In a mixed deployment, an old binary continues writing the legacy fields while a new binary writes the side table. The migration lease coordinates migration runners. It does not fence legacy writers.

## Prepare and upgrade

1. Identify the exact store, database and namespace. Record the deployed binaries and the SurrealDB version. For an embedded installation, use the effective `SURREALDB_DATA_DIR` or local `SURREALDB_URL` path and the matching database and namespace settings. For HTTP deployments, apply this procedure to each tenant namespace in scope. [Storage configuration](../../README.md#configuration)
2. Stop request serving and all other writers for that store or namespace. Include every replica, stdio host, one-shot client, hook, watcher, and lifecycle worker. Prevent supervisors from restarting them. Keep the fence through readiness and until every process serving the namespace runs the new binary. For embedded stores, wait for graceful shutdown and release of the store lock before copying the files.
3. Preserve a recovery point before opening the store with a new binary. For a closed SurrealKV directory, copy its full tree to a fresh protected destination and compare relative file paths and SHA-256 digests. Restore a **second copy** into a disposable path and open it with an engine compatible with the backup. Confirm the expected namespace, representative original facts and heat values. Keep the verified backup offline. For remote stores, use the established backup and restore procedure on an isolated destination with the same namespace and database layout. [HTTP restore drill](RESTORE_DRILL.md)
4. With the new binary and the production connection settings, run the pre-start maintenance command for your profile. This applies the catalogs without starting application servers or workers. Use the SurrealDB 4 upgrade instructions if the store itself also requires an engine conversion.

   ```bash
   # stdio's primary store
   memory_mcp surrealdb-v4-maintenance --role primary

   # HTTP Registry and ready tenant stores
   memory_mcp_http surrealdb-v4-maintenance
   ```

   Confirm migration `script_migration:057_fact_access_isolation_surql` reports `applied` in the application namespace. In the database administrator's SurrealQL session, confirm representative `fact_access` rows match the legacy count, timestamp and source episode. Include a fact with zero count and a timestamp if present. A cold fact with zero count and no timestamp has no row. Retain the backup and deployment records as evidence of the recovery point.
5. Resume serving with the new binaries only. On migration or verification failure, keep the fence and record the error. Repair the named database issue or restore the verified backup. The migration can resume from its committed cursor. A conflicting side row requires investigation before retrying.

## Roll back using the verified backup

Stop all application processes and background writers again. Preserve the failed or upgraded store separately. Restore the verified pre-upgrade recovery point into a fresh directory or remote destination, compare it with the backup's manifest before opening it where applicable, and verify the original data and access values there. Point the compatible predecessor deployment at the restored location. Traffic resumes after validation, using the backup's recovery point. Changes made after that recovery point, including writes beyond access heat, are lost in the restored copy unless separately recovered.

Use a predecessor that supports the backup's storage engine and on-disk format. Migration 057 does not convert between SurrealDB engine formats. [SurrealDB 4 upgrade](../../README.md#upgrade-to-v1400)

## Roll back while preserving post-upgrade writes

This route copies final side-table heat to the legacy fields. Keep all application writers stopped until reconciliation and old-binary verification are complete. Take a fresh fenced backup of the upgraded store before changing it. Use a SurrealDB 4 administrator's SurrealQL session or SDK bound to the precise namespace and database. For embedded stores without an operator SQL connection, use the verified backup route, or rehearse the SQL operation on a disposable copy using a compatible SurrealKV client before using it on the closed store. The Memory MCP data plane exposes no arbitrary SQL or migration rollback command.

1. Read the first page of keys in record order.

   ```sql
   SELECT id, record::id(id) AS fact_key FROM fact_access ORDER BY id LIMIT 128;
   ```

2. Bind `fact_access_rollback_keys` to the page's `fact_key` strings in a new query and execute [the rollback transaction](../../crates/memory-mcp/operations/fact_access_rollback_batch.surql) as a single script. In an administrator's SQL console, prepend `LET $fact_access_rollback_keys = ['key1', 'key2'];` to the script, using the exact keys from the page as SurrealQL string literals. An SDK can bind the array directly. Require success from every statement and the commit. The transaction checks the migration status and verifies each fact's identity and source episode before assigning the exact side count and timestamp. A mismatch aborts the page.
3. After the commit, save the last `fact_key` as the external cursor. Keep the last committed cursor, page keys and total processed count in the operator's recovery record. If the outcome of a commit is uncertain, repeat the same page while fenced. The operation assigns values and can be replayed without increasing counters.
4. Repeat from the committed cursor until the query returns an empty page. Bind `after` to the last committed string key.

   ```sql
   SELECT id, record::id(id) AS fact_key FROM fact_access
   WHERE id > type::record('fact_access', $after) ORDER BY id LIMIT 128;
   ```

5. Check the unique processed row count against `SELECT count() AS total FROM fact_access GROUP ALL;` while still fenced. Close and reopen the reconciled store and verify the legacy counter and exact last-access timestamp on known records, including one that accrued heat after the switch. Confirm its source and other fact fields survived. Start only the compatible predecessor while traffic remains fenced and verify its reads before resuming operations. Keep both recovery points.

The rollback transaction leaves the side table and migration ledger in place. Once a predecessor has written to the legacy fields, fence it again and arrange a fresh reconciliation and verified upgrade before starting a 057 binary on that store. The applied marker alone cannot establish that the legacy and side values agree after an old writer has run.

## Disposable native verification

The integration test exercises application upgrade and access writes, an offline byte-verified backup, a reopen, the exact operator rollback transaction with a 128-key first page and deterministic replay, and recovery from a distinct copy of the backup. It simulates the predecessor state on the same SurrealKV engine by using an empty additive schema without the 057 ledger entry. The test checks the legacy values directly and does not boot the preceding binary or exercise a remote deployment.

```bash
cargo test -p memory_mcp --test fact_access_native_upgrade --locked
```
