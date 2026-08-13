# ADR-0011: Use append-only automatic database migrations

## Status

Accepted

Amended by ADR-0038: startup upgrades only the one Active Namespace; an inactive
namespace is upgraded when it is later selected. Fresh default `main` does not
automatically discover or transfer legacy `org` data.

## Context

Installed databases may have been created by older application versions and contain durable user memory. The current startup path applies pending migrations to every configured namespace, recording each applied script with a checksum. Editing an already applied script causes checksum validation to fail and can prevent the application from starting.

## Decision

Released or applied migration files are immutable: their names, ordering identity, and contents must never be edited, deleted, or repurposed. Every schema or data change is introduced by a new, monotonically ordered migration.

On upgrade, the application automatically applies all pending migrations in deterministic order before serving requests. As amended by ADR-0038, this applies only to the process's Active Namespace; an inactive namespace is upgraded when it is later selected. A current application version must upgrade every explicitly supported older database version without data loss. The pre-stable `org`→`main` selection break requires explicit `SURREALDB_NAMESPACE=org` for old data and performs no discovery or transfer. Migrations must be restart-safe, preserve existing records and provenance, and tolerate legacy records that do not contain newly introduced optional fields. If migration cannot complete safely, startup fails before the application serves against a partially upgraded schema.

## Consequences

- `Claim` and `ClaimRelation` storage must be added by new migrations; historical migration files remain untouched.
- Upgrade compatibility requires fixtures or snapshots representing supported older database versions.
- Migration tests must cover both a fresh database and sequential upgrade from supported historical schemas.
- Destructive schema changes require an additive expand-migrate-contract sequence rather than rewriting migration history.
