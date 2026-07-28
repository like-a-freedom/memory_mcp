# ADR-0011: Use append-only automatic database migrations

## Status

Accepted

## Context

Installed databases may have been created by older application versions and contain durable user memory. The current startup path applies pending migrations to every configured namespace, recording each applied script with a checksum. Editing an already applied script causes checksum validation to fail and can prevent the application from starting.

## Decision

Released or applied migration files are immutable: their names, ordering identity, and contents must never be edited, deleted, or repurposed. Every schema or data change is introduced by a new, monotonically ordered migration.

On upgrade, the application automatically applies all pending migrations in deterministic order to every configured namespace before serving requests. A current application version must upgrade every explicitly supported older database version without manual configuration or data loss. Migrations must be restart-safe, preserve existing records and provenance, and tolerate legacy records that do not contain newly introduced optional fields. If migration cannot complete safely, startup fails before the application serves against a partially upgraded schema.

## Consequences

- `Claim` and `ClaimRelation` storage must be added by new migrations; historical migration files remain untouched.
- Upgrade compatibility requires fixtures or snapshots representing supported older database versions.
- Migration tests must cover both a fresh database and sequential upgrade from supported historical schemas.
- Destructive schema changes require an additive expand-migrate-contract sequence rather than rewriting migration history.
