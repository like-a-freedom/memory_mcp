use std::collections::BTreeMap;
use std::future::Future;
use std::hash::BuildHasher;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::MemoryError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServerVersion {
    pub(crate) major: u64,
    pub(crate) prerelease: bool,
    pub(crate) display: String,
}

impl ServerVersion {
    pub(crate) fn new(major: u64, prerelease: bool, display: impl Into<String>) -> Self {
        Self {
            major,
            prerelease,
            display: display.into(),
        }
    }
}

pub(crate) fn parse_server_version(value: &str) -> Result<ServerVersion, MemoryError> {
    let invalid = || {
        MemoryError::ConfigInvalid(format!(
            "SurrealDB server returned an invalid semantic version `{value}`"
        ))
    };

    if value.is_empty() || value.trim() != value {
        return Err(invalid());
    }

    let without_build = value.split_once('+').map_or(value, |(version, build)| {
        if build.is_empty() || !valid_dot_identifiers(build, false) {
            return "";
        }
        version
    });
    if without_build.is_empty() {
        return Err(invalid());
    }

    let (core, prerelease) = match without_build.split_once('-') {
        Some((core, prerelease)) => {
            if prerelease.is_empty() || !valid_dot_identifiers(prerelease, true) {
                return Err(invalid());
            }
            (core, true)
        }
        None => (without_build, false),
    };

    let mut components = core.split('.');
    let major = parse_numeric_component(components.next().ok_or_else(invalid)?)?;
    parse_numeric_component(components.next().ok_or_else(invalid)?)?;
    parse_numeric_component(components.next().ok_or_else(invalid)?)?;
    if components.next().is_some() {
        return Err(invalid());
    }

    Ok(ServerVersion::new(major, prerelease, value))
}

fn parse_numeric_component(component: &str) -> Result<u64, MemoryError> {
    if component.is_empty()
        || (component.len() > 1 && component.starts_with('0'))
        || !component.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(MemoryError::ConfigInvalid(format!(
            "SurrealDB server returned an invalid semantic version component `{component}`"
        )));
    }
    component.parse().map_err(|_| {
        MemoryError::ConfigInvalid(format!(
            "SurrealDB server returned an invalid semantic version component `{component}`"
        ))
    })
}

fn valid_dot_identifiers(value: &str, reject_numeric_leading_zero: bool) -> bool {
    value.split('.').all(|identifier| {
        !identifier.is_empty()
            && identifier
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && !(reject_numeric_leading_zero
                && identifier.len() > 1
                && identifier.starts_with('0')
                && identifier.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerCompatibility {
    LegacyCompatible,
    V4Maintenance,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DatabaseProvenance {
    FreshV4,
    PreExisting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DatabaseIdentity {
    pub(crate) namespace: String,
    pub(crate) database: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MaintenanceKey {
    pub(crate) namespace: String,
    pub(crate) database: String,
    pub(crate) revision: &'static str,
}

impl MaintenanceKey {
    fn new(identity: DatabaseIdentity) -> Self {
        Self {
            namespace: identity.namespace,
            database: identity.database,
            revision: MAINTENANCE_REVISION,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ClaimState {
    Active,
    Ambiguous,
    Failed,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IndexProgress {
    Pending,
    Intent,
    Submitted,
    Verified,
    Ambiguous,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MaintenanceRecord {
    pub(crate) key: MaintenanceKeyRecord,
    pub(crate) role: String,
    pub(crate) provenance: DatabaseProvenance,
    pub(crate) server_version: String,
    pub(crate) owner_id: String,
    pub(crate) attempt_id: String,
    pub(crate) state: ClaimState,
    pub(crate) current_index: Option<String>,
    pub(crate) indexes: BTreeMap<String, IndexProgress>,
    pub(crate) updated_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MaintenanceKeyRecord {
    pub(crate) namespace: String,
    pub(crate) database: String,
    pub(crate) revision: String,
}

impl From<&MaintenanceKey> for MaintenanceKeyRecord {
    fn from(key: &MaintenanceKey) -> Self {
        Self {
            namespace: key.namespace.clone(),
            database: key.database.clone(),
            revision: key.revision.to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexBuildState {
    Ready,
    Building,
    Failed,
    Missing,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexInspection {
    pub(crate) definition_matches: bool,
    pub(crate) state: IndexBuildState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryAuthorization {
    None,
    ConfirmNoPriorSubmitterCanResume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum V4CatalogIntent {
    Startup,
    PreStartMaintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceOutcome {
    SkippedLegacyCompatible,
    Completed,
}

pub(crate) const MAINTENANCE_REVISION: &str = "surrealdb_4_index_rebuild_v2";
const REBUILD_TIMEOUT: Duration = Duration::from_secs(120);
const REBUILD_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[async_trait]
pub(crate) trait V4MaintenancePort: Send + Sync {
    async fn server_version(&self) -> Result<ServerVersion, MemoryError>;
    async fn database_identity(&self) -> Result<DatabaseIdentity, MemoryError>;
    async fn database_is_empty(&self) -> Result<bool, MemoryError>;
    async fn read_provenance(&self) -> Result<Option<DatabaseProvenance>, MemoryError>;
    async fn create_provenance(&self, provenance: DatabaseProvenance) -> Result<bool, MemoryError>;
    async fn read_claim(
        &self,
        key: &MaintenanceKey,
    ) -> Result<Option<MaintenanceRecord>, MemoryError>;
    async fn create_claim(&self, record: &MaintenanceRecord) -> Result<bool, MemoryError>;
    async fn replace_claim(
        &self,
        expected_attempt_id: &str,
        record: &MaintenanceRecord,
    ) -> Result<bool, MemoryError>;
    async fn save_claim(
        &self,
        expected_attempt_id: &str,
        record: &MaintenanceRecord,
    ) -> Result<bool, MemoryError>;
    async fn inspect_index(&self, index: ManagedIndex) -> Result<IndexInspection, MemoryError>;
    async fn rebuild_index(&self, index: ManagedIndex) -> Result<(), MemoryError>;
}

pub(crate) async fn establish_database_provenance<P: V4MaintenancePort>(
    port: &P,
    provenance: DatabaseProvenance,
) -> Result<ServerCompatibility, MemoryError> {
    let version = port.server_version().await?;
    match classify_server_version(&version)? {
        ServerCompatibility::LegacyCompatible => Ok(ServerCompatibility::LegacyCompatible),
        ServerCompatibility::V4Maintenance => {
            match port.read_provenance().await? {
                Some(existing) if existing == provenance => {}
                Some(_) => {
                    return Err(MemoryError::Unavailable(
                        "database provenance conflicts with the durable pre-migration record"
                            .into(),
                    ));
                }
                None => {
                    if provenance == DatabaseProvenance::FreshV4
                        && !port.database_is_empty().await?
                    {
                        return Err(MemoryError::Unavailable(
                            "cannot record fresh SurrealDB 4 provenance for a database that is not empty; run the pre-start maintenance command after restoring or migrating the database".into(),
                        ));
                    }
                    if !port.create_provenance(provenance.clone()).await? {
                        let existing = port.read_provenance().await?;
                        if existing != Some(provenance) {
                            return Err(MemoryError::Unavailable(
                                "another process recorded conflicting database provenance".into(),
                            ));
                        }
                    }
                }
            }
            Ok(ServerCompatibility::V4Maintenance)
        }
    }
}

#[cfg(test)]
pub(crate) async fn preflight_v4_catalog<P: V4MaintenancePort>(
    port: &P,
    role: DatabaseRole,
    is_remote: bool,
    allow_remote_v4_index_rebuild: bool,
) -> Result<ServerCompatibility, MemoryError> {
    preflight_v4_catalog_with_intent(
        port,
        role,
        is_remote,
        allow_remote_v4_index_rebuild,
        V4CatalogIntent::Startup,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn preflight_v4_catalog_with_intent<P: V4MaintenancePort>(
    port: &P,
    role: DatabaseRole,
    is_remote: bool,
    allow_remote_v4_index_rebuild: bool,
    intent: V4CatalogIntent,
) -> Result<ServerCompatibility, MemoryError> {
    preflight_v4_catalog_with_recovery(
        port,
        role,
        is_remote,
        allow_remote_v4_index_rebuild,
        intent,
        RecoveryAuthorization::None,
    )
    .await
}

pub(crate) async fn preflight_v4_catalog_with_recovery<P: V4MaintenancePort>(
    port: &P,
    role: DatabaseRole,
    is_remote: bool,
    allow_remote_v4_index_rebuild: bool,
    intent: V4CatalogIntent,
    recovery: RecoveryAuthorization,
) -> Result<ServerCompatibility, MemoryError> {
    let version = port.server_version().await?;
    let compatibility = classify_server_version(&version)?;
    if compatibility == ServerCompatibility::LegacyCompatible {
        return Ok(compatibility);
    }

    if is_remote && !allow_remote_v4_index_rebuild {
        return Err(MemoryError::ConfigInvalid(
            "remote SurrealDB 4 catalog changes require SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD=true after all datastore-sharing nodes are on v4 and older writers are paused".into(),
        ));
    }

    let identity = port.database_identity().await?;
    let provenance = match port.read_provenance().await? {
        Some(provenance) => provenance,
        None => {
            let database_is_empty = port.database_is_empty().await?;
            if intent == V4CatalogIntent::Startup && !database_is_empty {
                return Err(MemoryError::Unavailable(
                    "SurrealDB 4 database has no recorded migration provenance; run the pre-start maintenance command before applying schema changes".into(),
                ));
            }
            if database_is_empty {
                DatabaseProvenance::FreshV4
            } else {
                DatabaseProvenance::PreExisting
            }
        }
    };

    let key = MaintenanceKey::new(identity);
    let prior_claim = port.read_claim(&key).await?;
    if let Some(claim) = &prior_claim {
        if claim.role != role_name(role) || claim.provenance != provenance {
            let message = if claim.state == ClaimState::Completed {
                "completed SurrealDB 4 maintenance record does not match the active database role or provenance"
            } else {
                "incomplete SurrealDB 4 maintenance claim belongs to a different role or provenance; refusing recovery"
            };
            return Err(MemoryError::Unavailable(message.into()));
        }
        if intent == V4CatalogIntent::Startup && provenance == DatabaseProvenance::PreExisting {
            match claim.state {
                ClaimState::Completed => {}
                ClaimState::Active => {
                    return Err(MemoryError::Unavailable(
                        "SurrealDB 4 maintenance is active in another process; wait for it to finish and retry".into(),
                    ));
                }
                _ => return Err(recovery_required_error()),
            }
        }
        if claim.state == ClaimState::Active && recovery == RecoveryAuthorization::None {
            return Err(MemoryError::Unavailable(
                "SurrealDB 4 maintenance is active in another process; wait for it to finish and retry. If that process stopped, verify no server-side index build is running before using explicit ambiguous recovery".into(),
            ));
        }
        if matches!(claim.state, ClaimState::Ambiguous | ClaimState::Failed)
            && recovery == RecoveryAuthorization::None
            && !failed_claim_is_safe_to_retry(
                port,
                claim,
                role,
                &provenance,
                managed_indexes_for_role(role),
            )
            .await?
        {
            return Err(recovery_required_error());
        }
    } else if intent == V4CatalogIntent::Startup && provenance == DatabaseProvenance::PreExisting {
        return Err(MemoryError::Unavailable(
            "SurrealDB 4 pre-existing database has no completed maintenance claim; run the pre-start maintenance command before applying schema changes".into(),
        ));
    }

    for index in managed_indexes_for_role(role) {
        let inspection = port.inspect_index(*index).await?;
        match inspection.state {
            IndexBuildState::Building => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has an active server-side build; wait for it to finish before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Failed => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has a failed server-side build; resolve it before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Unknown => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has an unknown server-side build state; inspect it before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Ready if !inspection.definition_matches => {
                return Err(index_definition_error(index));
            }
            IndexBuildState::Ready | IndexBuildState::Missing => {}
        }
    }

    Ok(compatibility)
}

#[cfg(any(test, feature = "streamable-http"))]
pub(crate) async fn run_v4_catalog_maintenance<P, F, Fut>(
    port: &P,
    role: DatabaseRole,
    is_remote: bool,
    allow_remote_v4_index_rebuild: bool,
    recovery: RecoveryAuthorization,
    apply_catalog: F,
) -> Result<MaintenanceOutcome, MemoryError>
where
    P: V4MaintenancePort,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), MemoryError>>,
{
    run_v4_catalog_maintenance_with_intent(
        port,
        role,
        is_remote,
        allow_remote_v4_index_rebuild,
        recovery,
        V4CatalogIntent::PreStartMaintenance,
        apply_catalog,
    )
    .await
}

pub(crate) async fn run_v4_catalog_maintenance_with_intent<P, F, Fut>(
    port: &P,
    role: DatabaseRole,
    is_remote: bool,
    allow_remote_v4_index_rebuild: bool,
    recovery: RecoveryAuthorization,
    intent: V4CatalogIntent,
    apply_catalog: F,
) -> Result<MaintenanceOutcome, MemoryError>
where
    P: V4MaintenancePort,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<(), MemoryError>>,
{
    let compatibility = preflight_v4_catalog_with_recovery(
        port,
        role,
        is_remote,
        allow_remote_v4_index_rebuild,
        intent,
        recovery,
    )
    .await?;
    if compatibility == ServerCompatibility::LegacyCompatible {
        apply_catalog().await?;
        return Ok(MaintenanceOutcome::SkippedLegacyCompatible);
    }

    let version = port.server_version().await?;
    let identity = port.database_identity().await?;
    let key = MaintenanceKey::new(identity);
    let provenance = match port.read_provenance().await? {
        Some(provenance) => provenance,
        None if port.database_is_empty().await? => DatabaseProvenance::FreshV4,
        None if intent == V4CatalogIntent::PreStartMaintenance => DatabaseProvenance::PreExisting,
        None => {
            return Err(MemoryError::Unavailable(
                "SurrealDB 4 database has no recorded migration provenance; run the pre-start maintenance command before applying schema changes".into(),
            ));
        }
    };
    let prior_claim = port.read_claim(&key).await?;
    if intent == V4CatalogIntent::Startup && provenance == DatabaseProvenance::PreExisting {
        match prior_claim.as_ref().map(|claim| claim.state) {
            Some(ClaimState::Completed) => {}
            Some(ClaimState::Active) => {
                return Err(MemoryError::Unavailable(
                    "SurrealDB 4 maintenance is active in another process; wait for it to finish and retry".into(),
                ));
            }
            Some(_) => return Err(recovery_required_error()),
            None => {
                return Err(MemoryError::Unavailable(
                    "SurrealDB 4 pre-existing database has no completed maintenance claim; run the pre-start maintenance command before applying schema changes".into(),
                ));
            }
        }
    }
    establish_database_provenance(port, provenance.clone()).await?;
    let indexes = managed_indexes_for_role(role);
    for index in indexes {
        let inspection = port.inspect_index(*index).await?;
        match inspection.state {
            IndexBuildState::Building => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has an active server-side build; wait for it to finish before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Failed => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has a failed server-side build; resolve it before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Unknown => {
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has an unknown server-side build state; inspect it before applying the schema catalog",
                    index.name
                )));
            }
            IndexBuildState::Ready if !inspection.definition_matches => {
                return Err(index_definition_error(index));
            }
            IndexBuildState::Ready | IndexBuildState::Missing => {}
        }
    }

    let mut claim = match prior_claim {
        Some(existing) if existing.state == ClaimState::Completed => {
            verify_completed_claim_identity(&existing, role, &provenance)?;
            let mut claim = new_claim(&key, role, provenance.clone(), &version, indexes);
            for (name, progress) in existing.indexes {
                if progress == IndexProgress::Verified {
                    claim.indexes.insert(name, progress);
                }
            }
            if !port.replace_claim(&existing.attempt_id, &claim).await? {
                return Err(MemoryError::Unavailable(
                    "completed maintenance claim changed before catalog admission; no schema changes were applied".into(),
                ));
            }
            claim
        }
        Some(existing) => {
            if existing.state == ClaimState::Active && recovery == RecoveryAuthorization::None {
                return Err(MemoryError::Unavailable(
                    "SurrealDB 4 maintenance is active in another process; wait for it to finish and retry. If that process stopped, verify no server-side index build is running before using explicit ambiguous recovery".into(),
                ));
            }
            let safe_retry = if recovery == RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume
            {
                false
            } else {
                failed_claim_is_safe_to_retry(port, &existing, role, &provenance, indexes).await?
            };
            if recovery != RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume && !safe_retry {
                return Err(recovery_required_error());
            }
            if existing.role != role_name(role) || existing.provenance != provenance {
                return Err(MemoryError::Unavailable(
                    "incomplete SurrealDB 4 maintenance claim belongs to a different role or provenance; refusing recovery".into(),
                ));
            }
            let mut claim = new_claim(&key, role, provenance.clone(), &version, indexes);
            for (name, progress) in existing.indexes {
                if progress == IndexProgress::Verified {
                    claim.indexes.insert(name, progress);
                }
            }
            if !port.replace_claim(&existing.attempt_id, &claim).await? {
                return Err(MemoryError::Unavailable(
                    "maintenance claim changed before catalog admission; no schema changes were applied".into(),
                ));
            }
            claim
        }
        None => {
            let mut claim = new_claim(&key, role, provenance.clone(), &version, indexes);
            if provenance == DatabaseProvenance::FreshV4 {
                for progress in claim.indexes.values_mut() {
                    *progress = IndexProgress::Verified;
                }
            }
            if !port.create_claim(&claim).await? {
                return Err(MemoryError::Unavailable(
                    "another submitter acquired the SurrealDB 4 maintenance claim; no schema changes were applied".into(),
                ));
            }
            claim
        }
    };

    for index in indexes {
        let inspection = match port.inspect_index(*index).await {
            Ok(inspection) => inspection,
            Err(error) => {
                mark_claim_failed_before_catalog(port, &mut claim).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` could not be inspected after acquiring the maintenance claim ({error}); no schema catalog was applied",
                    index.name
                )));
            }
        };
        let error = match inspection.state {
            IndexBuildState::Building => Some(format!(
                "index `{}` has an active server-side build",
                index.name
            )),
            IndexBuildState::Failed => Some(format!(
                "index `{}` has a failed server-side build",
                index.name
            )),
            IndexBuildState::Unknown => Some(format!(
                "index `{}` has an unknown server-side build state",
                index.name
            )),
            IndexBuildState::Ready if !inspection.definition_matches => {
                Some(index_definition_error(index).to_string())
            }
            IndexBuildState::Ready | IndexBuildState::Missing => None,
        };
        if let Some(error) = error {
            mark_claim_failed_before_catalog(port, &mut claim).await?;
            return Err(MemoryError::Unavailable(format!(
                "{error}; no schema catalog was applied"
            )));
        }
    }

    if let Err(error) = apply_catalog().await {
        claim.state = ClaimState::Ambiguous;
        claim.current_index = None;
        claim.updated_at_ms = now_ms();
        if !port.save_claim(&claim.attempt_id, &claim).await? {
            return Err(MemoryError::Unavailable(format!(
                "schema catalog may have applied partially ({error}) and its ambiguous outcome could not be durably recorded; keep readiness closed"
            )));
        }
        return Err(MemoryError::Unavailable(format!(
            "schema catalog may have applied partially ({error}); readiness remains closed. Stop prior submitters, inspect the database, then rerun the pre-start maintenance command with explicit ambiguous-recovery confirmation"
        )));
    }

    for index in indexes {
        let inspection = match port.inspect_index(*index).await {
            Ok(inspection) => inspection,
            Err(error) => {
                mark_claim_ambiguous(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` could not be inspected after the schema catalog ({error}); keep readiness closed",
                    index.name
                )));
            }
        };
        if !inspection.definition_matches || inspection.state == IndexBuildState::Missing {
            mark_claim_failed(port, &mut claim, index.name).await?;
            return Err(index_definition_error(index));
        }
        match inspection.state {
            IndexBuildState::Ready => {}
            IndexBuildState::Failed => {
                mark_claim_failed(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` reports a failed build after the schema catalog; keep readiness closed",
                    index.name
                )));
            }
            IndexBuildState::Building | IndexBuildState::Unknown => {
                mark_claim_ambiguous(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` has active or unknown server-side build state after the schema catalog; keep readiness closed",
                    index.name
                )));
            }
            IndexBuildState::Missing => {
                mark_claim_failed(port, &mut claim, index.name).await?;
                return Err(index_definition_error(index));
            }
        }
    }

    if provenance == DatabaseProvenance::FreshV4 {
        for progress in claim.indexes.values_mut() {
            *progress = IndexProgress::Verified;
        }
        claim.state = ClaimState::Completed;
        claim.current_index = None;
        claim.updated_at_ms = now_ms();
        if !port.save_claim(&claim.attempt_id, &claim).await? {
            return Err(MemoryError::Unavailable(
                "fresh-v4 catalog completion could not be durably recorded; keep readiness closed"
                    .into(),
            ));
        }
    } else {
        execute_maintenance(port, claim, indexes).await?;
    }

    Ok(MaintenanceOutcome::Completed)
}

async fn execute_maintenance<P: V4MaintenancePort>(
    port: &P,
    mut claim: MaintenanceRecord,
    indexes: &[ManagedIndex],
) -> Result<(), MemoryError> {
    for index in indexes {
        if claim.indexes.get(index.name) == Some(&IndexProgress::Verified) {
            continue;
        }
        claim.current_index = Some(index.name.to_owned());
        claim
            .indexes
            .insert(index.name.to_owned(), IndexProgress::Intent);
        persist_claim(port, &mut claim).await?;

        let inspection = port.inspect_index(*index).await?;
        if !inspection.definition_matches || inspection.state == IndexBuildState::Missing {
            mark_claim_failed(port, &mut claim, index.name).await?;
            return Err(index_definition_error(index));
        }
        if matches!(
            inspection.state,
            IndexBuildState::Building | IndexBuildState::Unknown
        ) {
            mark_claim_ambiguous(port, &mut claim, index.name).await?;
            return Err(MemoryError::Unavailable(format!(
                "index `{}` has active or unknown server-side build state; no rebuild was submitted",
                index.name
            )));
        }

        claim
            .indexes
            .insert(index.name.to_owned(), IndexProgress::Submitted);
        persist_claim(port, &mut claim).await?;
        let rebuild = tokio::time::timeout(REBUILD_TIMEOUT, port.rebuild_index(*index)).await;
        match rebuild {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                mark_claim_ambiguous(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "REBUILD INDEX `{}` returned an uncertain result ({error}); readiness remains closed. Stop prior submitters, confirm no server-side build is active, then rerun the maintenance command with explicit ambiguous-recovery confirmation",
                    index.name
                )));
            }
            Err(_) => {
                mark_claim_ambiguous(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "REBUILD INDEX `{}` timed out; the server may still be working. Stop prior submitters, confirm no server-side build is active, then rerun the maintenance command with explicit ambiguous-recovery confirmation",
                    index.name
                )));
            }
        }

        match wait_for_index_ready(port, *index).await {
            Ok(()) => {
                claim
                    .indexes
                    .insert(index.name.to_owned(), IndexProgress::Verified);
                claim.current_index = None;
                persist_claim(port, &mut claim).await?;
            }
            Err(IndexWaitFailure::State(IndexBuildState::Failed)) => {
                mark_claim_failed(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` reported a failed build after rebuild; readiness remains closed",
                    index.name
                )));
            }
            Err(failure) => {
                mark_claim_ambiguous(port, &mut claim, index.name).await?;
                return Err(MemoryError::Unavailable(format!(
                    "index `{}` could not be verified after rebuild ({failure}); readiness remains closed",
                    index.name
                )));
            }
        }
    }

    claim.current_index = None;
    claim.state = ClaimState::Completed;
    claim.updated_at_ms = now_ms();
    if !port.save_claim(&claim.attempt_id, &claim).await? {
        return Err(MemoryError::Unavailable(
            "the maintenance result could not be durably recorded; the index outcome is ambiguous and readiness remains closed".into(),
        ));
    }
    Ok(())
}

enum IndexWaitFailure {
    State(IndexBuildState),
    Inspection(MemoryError),
}

impl std::fmt::Display for IndexWaitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State(state) => write!(formatter, "state: {state:?}"),
            Self::Inspection(error) => write!(formatter, "inspection failed: {error}"),
        }
    }
}

async fn wait_for_index_ready<P: V4MaintenancePort>(
    port: &P,
    index: ManagedIndex,
) -> Result<(), IndexWaitFailure> {
    let deadline = Instant::now() + REBUILD_TIMEOUT;
    loop {
        let inspection = port
            .inspect_index(index)
            .await
            .map_err(IndexWaitFailure::Inspection)?;
        if !inspection.definition_matches || inspection.state == IndexBuildState::Missing {
            return Err(IndexWaitFailure::State(IndexBuildState::Missing));
        }
        match inspection.state {
            IndexBuildState::Ready => return Ok(()),
            IndexBuildState::Failed => {
                return Err(IndexWaitFailure::State(IndexBuildState::Failed));
            }
            IndexBuildState::Building if Instant::now() < deadline => {
                tokio::time::sleep(REBUILD_POLL_INTERVAL).await;
            }
            IndexBuildState::Building => {
                return Err(IndexWaitFailure::State(IndexBuildState::Building));
            }
            IndexBuildState::Missing => {
                return Err(IndexWaitFailure::State(IndexBuildState::Missing));
            }
            IndexBuildState::Unknown => {
                return Err(IndexWaitFailure::State(IndexBuildState::Unknown));
            }
        }
    }
}

async fn failed_claim_is_safe_to_retry<P: V4MaintenancePort>(
    port: &P,
    record: &MaintenanceRecord,
    role: DatabaseRole,
    provenance: &DatabaseProvenance,
    indexes: &[ManagedIndex],
) -> Result<bool, MemoryError> {
    if record.state != ClaimState::Failed
        || record.role != role_name(role)
        || &record.provenance != provenance
    {
        return Ok(false);
    }
    let Some(index_name) = record.current_index.as_deref() else {
        return Ok(true);
    };
    let Some(index) = indexes.iter().find(|index| index.name == index_name) else {
        return Ok(false);
    };
    let inspection = port.inspect_index(*index).await?;
    Ok(inspection.definition_matches && inspection.state == IndexBuildState::Ready)
}

fn verify_completed_claim_identity(
    record: &MaintenanceRecord,
    role: DatabaseRole,
    provenance: &DatabaseProvenance,
) -> Result<(), MemoryError> {
    if record.role != role_name(role) || &record.provenance != provenance {
        return Err(MemoryError::Unavailable(
            "completed SurrealDB 4 maintenance record does not match the active database role or provenance".into(),
        ));
    }
    Ok(())
}

async fn persist_claim<P: V4MaintenancePort>(
    port: &P,
    claim: &mut MaintenanceRecord,
) -> Result<(), MemoryError> {
    claim.updated_at_ms = now_ms();
    if !port.save_claim(&claim.attempt_id, claim).await? {
        return Err(MemoryError::Unavailable(
            "maintenance claim ownership changed; refusing to submit or record further index work"
                .into(),
        ));
    }
    Ok(())
}

async fn mark_claim_failed_before_catalog<P: V4MaintenancePort>(
    port: &P,
    claim: &mut MaintenanceRecord,
) -> Result<(), MemoryError> {
    claim.state = ClaimState::Failed;
    claim.current_index = None;
    persist_claim(port, claim).await
}

async fn mark_claim_ambiguous<P: V4MaintenancePort>(
    port: &P,
    claim: &mut MaintenanceRecord,
    index_name: &str,
) -> Result<(), MemoryError> {
    claim.state = ClaimState::Ambiguous;
    claim.current_index = Some(index_name.to_owned());
    claim
        .indexes
        .insert(index_name.to_owned(), IndexProgress::Ambiguous);
    persist_claim(port, claim).await
}

async fn mark_claim_failed<P: V4MaintenancePort>(
    port: &P,
    claim: &mut MaintenanceRecord,
    index_name: &str,
) -> Result<(), MemoryError> {
    claim.state = ClaimState::Failed;
    claim.current_index = Some(index_name.to_owned());
    claim
        .indexes
        .insert(index_name.to_owned(), IndexProgress::Failed);
    persist_claim(port, claim).await
}

fn new_claim(
    key: &MaintenanceKey,
    role: DatabaseRole,
    provenance: DatabaseProvenance,
    version: &ServerVersion,
    indexes: &[ManagedIndex],
) -> MaintenanceRecord {
    let attempt_id = new_attempt_id();
    MaintenanceRecord {
        key: MaintenanceKeyRecord::from(key),
        role: role_name(role).to_owned(),
        provenance,
        server_version: version.display.clone(),
        owner_id: format!("pid-{}", std::process::id()),
        attempt_id,
        state: ClaimState::Active,
        current_index: None,
        indexes: indexes
            .iter()
            .map(|index| (index.name.to_owned(), IndexProgress::Pending))
            .collect(),
        updated_at_ms: now_ms(),
    }
}

fn new_attempt_id() -> String {
    static NEXT_ATTEMPT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static HASHER: std::sync::OnceLock<std::collections::hash_map::RandomState> =
        std::sync::OnceLock::new();
    let sequence = NEXT_ATTEMPT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let timestamp = now_ms();
    let process_id = std::process::id();
    let nonce = HASHER
        .get_or_init(std::collections::hash_map::RandomState::new)
        .hash_one((process_id, timestamp, sequence));
    format!("{process_id}-{timestamp}-{sequence}-{nonce:016x}")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn role_name(role: DatabaseRole) -> &'static str {
    match role {
        DatabaseRole::PrimaryMemory => "primary_memory",
        #[cfg(any(test, feature = "streamable-http"))]
        DatabaseRole::HttpTenant => "http_tenant",
        #[cfg(any(test, feature = "streamable-http"))]
        DatabaseRole::Registry => "registry",
    }
}

fn recovery_required_error() -> MemoryError {
    MemoryError::Unavailable(
        "an incomplete SurrealDB 4 maintenance claim exists; readiness remains closed. Stop prior submitters and confirm no server-side rebuild is active before using --recover-ambiguous --confirm-no-prior-submitter-can-resume".into(),
    )
}

fn index_definition_error(index: &ManagedIndex) -> MemoryError {
    MemoryError::Unavailable(format!(
        "managed index `{}` on table `{}` is missing or does not match the expected definition; no rebuild was accepted",
        index.name, index.table
    ))
}

pub(crate) fn classify_server_version(
    version: &ServerVersion,
) -> Result<ServerCompatibility, MemoryError> {
    if version.prerelease {
        return Err(MemoryError::ConfigInvalid(format!(
            "SurrealDB prerelease `{}` is not supported for automatic startup",
            version.display
        )));
    }

    match version.major {
        3 => Ok(ServerCompatibility::LegacyCompatible),
        4 => Ok(ServerCompatibility::V4Maintenance),
        major => Err(MemoryError::ConfigInvalid(format!(
            "SurrealDB major version {major} is unsupported; expected stable 3.x or 4.x"
        ))),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DatabaseRole {
    PrimaryMemory,
    #[cfg(any(test, feature = "streamable-http"))]
    HttpTenant,
    #[cfg(any(test, feature = "streamable-http"))]
    Registry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagedIndex {
    pub(crate) name: &'static str,
    pub(crate) table: &'static str,
    pub(crate) definition: &'static str,
}

const MEMORY_INDEXES: &[ManagedIndex] = &[
    ManagedIndex {
        name: "edge_temporal_idx",
        table: "edge",
        definition: "DEFINE INDEX edge_temporal_idx ON edge COLUMNS in, out, t_valid, t_invalid",
    },
    ManagedIndex {
        name: "fact_t_valid",
        table: "fact",
        definition: "DEFINE INDEX fact_t_valid ON TABLE fact FIELDS t_valid",
    },
    ManagedIndex {
        name: "episode_t_ref",
        table: "episode",
        definition: "DEFINE INDEX episode_t_ref ON TABLE episode FIELDS t_ref",
    },
    ManagedIndex {
        name: "edge_t_valid",
        table: "edge",
        definition: "DEFINE INDEX edge_t_valid ON TABLE edge FIELDS t_valid",
    },
    ManagedIndex {
        name: "claim_relation_left_active_idx",
        table: "claim_relation",
        definition: "DEFINE INDEX claim_relation_left_active_idx ON claim_relation COLUMNS left_claim_id, t_invalid_ingested",
    },
    ManagedIndex {
        name: "claim_relation_right_active_idx",
        table: "claim_relation",
        definition: "DEFINE INDEX claim_relation_right_active_idx ON claim_relation COLUMNS right_claim_id, t_invalid_ingested",
    },
    ManagedIndex {
        name: "claim_job_lease_idx",
        table: "claim_job",
        definition: "DEFINE INDEX claim_job_lease_idx ON claim_job COLUMNS status, lease_expires_at, job_id",
    },
    ManagedIndex {
        name: "inbox_revision_state_lease",
        table: "inbox_revision",
        definition: "DEFINE INDEX inbox_revision_state_lease ON inbox_revision FIELDS state, lease_expires_at",
    },
    ManagedIndex {
        name: "entity_aliases",
        table: "entity",
        definition: "DEFINE INDEX entity_aliases ON TABLE entity FIELDS aliases.*",
    },
    ManagedIndex {
        name: "community_members",
        table: "community",
        definition: "DEFINE INDEX community_members ON TABLE community FIELDS member_entities.*",
    },
    ManagedIndex {
        name: "community_summary_search",
        table: "community",
        definition: "DEFINE INDEX community_summary_search ON TABLE community COLUMNS summary FULLTEXT ANALYZER memory_fts BM25(1.2,0.75)",
    },
    ManagedIndex {
        name: "fact_embedding_hnsw",
        table: "fact",
        definition: "DEFINE INDEX fact_embedding_hnsw ON TABLE fact FIELDS embedding HNSW DIMENSION __FACT_EMBEDDING_DIMENSION__ DIST EUCLIDEAN TYPE F32 EFC 150 M 12 M0 24 LM 0.40242960438184466f",
    },
];

#[cfg(test)]
const REGISTRY_AUDIT_INDEX: ManagedIndex = ManagedIndex {
    name: "idx_audit_event_account",
    table: "audit_event",
    definition: "DEFINE INDEX IF NOT EXISTS idx_audit_event_account ON audit_event FIELDS account_id, occurred_at",
};

pub(crate) fn managed_indexes_for_role(role: DatabaseRole) -> &'static [ManagedIndex] {
    match role {
        DatabaseRole::PrimaryMemory => MEMORY_INDEXES,
        #[cfg(any(test, feature = "streamable-http"))]
        DatabaseRole::HttpTenant => MEMORY_INDEXES,
        #[cfg(any(test, feature = "streamable-http"))]
        DatabaseRole::Registry => &[],
    }
}

#[cfg(test)]
pub(crate) fn registry_audit_index() -> ManagedIndex {
    REGISTRY_AUDIT_INDEX
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::{
        ClaimState, DatabaseIdentity, DatabaseProvenance, DatabaseRole, IndexBuildState,
        IndexInspection, IndexProgress, MAINTENANCE_REVISION, MaintenanceKey, MaintenanceKeyRecord,
        MaintenanceOutcome, MaintenanceRecord, ManagedIndex, RecoveryAuthorization,
        ServerCompatibility, ServerVersion, V4CatalogIntent, V4MaintenancePort,
        classify_server_version, establish_database_provenance, managed_indexes_for_role,
        parse_server_version, preflight_v4_catalog, preflight_v4_catalog_with_intent,
        registry_audit_index, run_v4_catalog_maintenance, run_v4_catalog_maintenance_with_intent,
    };
    use crate::error::MemoryError;

    struct FakePort {
        state: Mutex<FakePortState>,
    }

    struct FakePortState {
        version: ServerVersion,
        database_is_empty: bool,
        database_empty_reads: usize,
        provenance: Option<DatabaseProvenance>,
        provenance_reads: usize,
        claim: Option<MaintenanceRecord>,
        fail_rebuild: bool,
        fail_post_rebuild_inspection: bool,
        fail_definition_on_post_catalog_inspection: Option<&'static str>,
        running_build: bool,
        unknown_build_state: bool,
        missing_until_catalog: Option<&'static str>,
        fail_completion_write: bool,
        start_build_when_claimed: bool,
        rebuild_calls: usize,
        inspections: HashMap<String, usize>,
        saved_records: Vec<MaintenanceRecord>,
        events: Vec<&'static str>,
    }

    impl FakePort {
        fn new(version: &str, provenance: Option<DatabaseProvenance>) -> Self {
            Self {
                state: Mutex::new(FakePortState {
                    version: parse_server_version(version).expect("valid test version"),
                    database_is_empty: false,
                    database_empty_reads: 0,
                    provenance,
                    provenance_reads: 0,
                    claim: None,
                    fail_rebuild: false,
                    fail_post_rebuild_inspection: false,
                    fail_definition_on_post_catalog_inspection: None,
                    running_build: false,
                    unknown_build_state: false,
                    missing_until_catalog: None,
                    fail_completion_write: false,
                    start_build_when_claimed: false,
                    rebuild_calls: 0,
                    inspections: HashMap::new(),
                    saved_records: Vec::new(),
                    events: Vec::new(),
                }),
            }
        }

        fn with_preexisting_database() -> Self {
            Self::new("4.0.1", Some(DatabaseProvenance::PreExisting))
        }

        fn rebuild_calls(&self) -> usize {
            self.state.lock().expect("fake port mutex").rebuild_calls
        }

        fn preflight_reads(&self) -> (usize, usize) {
            let state = self.state.lock().expect("fake port mutex");
            (state.provenance_reads, state.database_empty_reads)
        }

        fn claim(&self) -> Option<MaintenanceRecord> {
            self.state.lock().expect("fake port mutex").claim.clone()
        }

        fn saved_records(&self) -> Vec<MaintenanceRecord> {
            self.state
                .lock()
                .expect("fake port mutex")
                .saved_records
                .clone()
        }

        fn set_fail_rebuild(&self, fail: bool) {
            self.state.lock().expect("fake port mutex").fail_rebuild = fail;
        }

        fn record_catalog(&self) {
            let mut state = self.state.lock().expect("fake port mutex");
            assert!(
                state
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.state == ClaimState::Active)
            );
            state.events.push("catalog");
            state.missing_until_catalog = None;
        }

        fn hide_index_until_catalog(&self, index_name: &'static str) {
            self.state
                .lock()
                .expect("fake port mutex")
                .missing_until_catalog = Some(index_name);
        }

        fn events(&self) -> Vec<&'static str> {
            self.state.lock().expect("fake port mutex").events.clone()
        }
    }

    async fn run_test_catalog_maintenance(
        port: &FakePort,
        role: DatabaseRole,
        recovery: RecoveryAuthorization,
    ) -> Result<MaintenanceOutcome, MemoryError> {
        run_v4_catalog_maintenance(port, role, false, false, recovery, || async { Ok(()) }).await
    }

    #[async_trait]
    impl V4MaintenancePort for FakePort {
        async fn server_version(&self) -> Result<ServerVersion, MemoryError> {
            Ok(self.state.lock().expect("fake port mutex").version.clone())
        }

        async fn database_identity(&self) -> Result<DatabaseIdentity, MemoryError> {
            Ok(DatabaseIdentity {
                namespace: "test_namespace".to_owned(),
                database: "test_database".to_owned(),
            })
        }

        async fn database_is_empty(&self) -> Result<bool, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            state.database_empty_reads += 1;
            Ok(state.database_is_empty)
        }

        async fn read_provenance(&self) -> Result<Option<DatabaseProvenance>, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            state.provenance_reads += 1;
            Ok(state.provenance.clone())
        }

        async fn create_provenance(
            &self,
            provenance: DatabaseProvenance,
        ) -> Result<bool, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            if state.provenance.is_some() {
                return Ok(false);
            }
            state.provenance = Some(provenance);
            Ok(true)
        }

        async fn read_claim(
            &self,
            _key: &MaintenanceKey,
        ) -> Result<Option<MaintenanceRecord>, MemoryError> {
            Ok(self.state.lock().expect("fake port mutex").claim.clone())
        }

        async fn create_claim(&self, record: &MaintenanceRecord) -> Result<bool, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            if state.claim.is_some() {
                return Ok(false);
            }
            state.events.push("claim");
            state.claim = Some(record.clone());
            if state.start_build_when_claimed {
                state.running_build = true;
            }
            Ok(true)
        }

        async fn replace_claim(
            &self,
            expected_attempt_id: &str,
            record: &MaintenanceRecord,
        ) -> Result<bool, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            if state
                .claim
                .as_ref()
                .is_none_or(|claim| claim.attempt_id != expected_attempt_id)
            {
                return Ok(false);
            }
            state.claim = Some(record.clone());
            Ok(true)
        }

        async fn save_claim(
            &self,
            expected_attempt_id: &str,
            record: &MaintenanceRecord,
        ) -> Result<bool, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            if state
                .claim
                .as_ref()
                .is_none_or(|claim| claim.attempt_id != expected_attempt_id)
                || (state.fail_completion_write && record.state == ClaimState::Completed)
            {
                return Ok(false);
            }
            state.claim = Some(record.clone());
            state.saved_records.push(record.clone());
            Ok(true)
        }

        async fn inspect_index(&self, index: ManagedIndex) -> Result<IndexInspection, MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            if state.missing_until_catalog == Some(index.name) {
                return Ok(IndexInspection {
                    definition_matches: false,
                    state: IndexBuildState::Missing,
                });
            }
            if state.fail_post_rebuild_inspection && state.rebuild_calls > 0 {
                return Err(MemoryError::Storage("inspection response lost".into()));
            }
            if state.running_build {
                return Ok(IndexInspection {
                    definition_matches: true,
                    state: IndexBuildState::Building,
                });
            }
            if state.unknown_build_state {
                return Ok(IndexInspection {
                    definition_matches: true,
                    state: IndexBuildState::Unknown,
                });
            }
            let failing_index = state.fail_definition_on_post_catalog_inspection;
            let inspected = state.inspections.entry(index.name.to_owned()).or_default();
            *inspected += 1;
            let inspection_count = *inspected;
            if failing_index == Some(index.name) && inspection_count == 4 {
                return Ok(IndexInspection {
                    definition_matches: false,
                    state: IndexBuildState::Missing,
                });
            }
            Ok(IndexInspection {
                definition_matches: true,
                state: IndexBuildState::Ready,
            })
        }

        async fn rebuild_index(&self, _index: ManagedIndex) -> Result<(), MemoryError> {
            let mut state = self.state.lock().expect("fake port mutex");
            state.events.push("rebuild");
            state.rebuild_calls += 1;
            if state.fail_rebuild {
                return Err(MemoryError::Storage(
                    "response lost after submission".into(),
                ));
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn recovery_refuses_a_claim_owned_by_another_role() {
        let port = FakePort::with_preexisting_database();
        let key = MaintenanceKey {
            namespace: "test_namespace".to_owned(),
            database: "test_database".to_owned(),
            revision: MAINTENANCE_REVISION,
        };
        let old_claim = MaintenanceRecord {
            key: MaintenanceKeyRecord::from(&key),
            role: "registry".to_owned(),
            provenance: DatabaseProvenance::PreExisting,
            server_version: "4.0.1".to_owned(),
            owner_id: "old-owner".to_owned(),
            attempt_id: "old-attempt".to_owned(),
            state: ClaimState::Ambiguous,
            current_index: None,
            indexes: BTreeMap::new(),
            updated_at_ms: 1,
        };
        port.state.lock().expect("fake port mutex").claim = Some(old_claim);

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume,
        )
        .await
        .expect_err("recovery must not change the role attached to an existing claim");

        assert!(error.to_string().contains("different role or provenance"));
        assert_eq!(port.rebuild_calls(), 0);
        assert_eq!(
            port.claim().expect("old claim remains").attempt_id,
            "old-attempt"
        );
    }

    #[tokio::test]
    async fn startup_refuses_preexisting_provenance_without_completed_claim() {
        let port = FakePort::with_preexisting_database();

        let result = run_v4_catalog_maintenance_with_intent(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            V4CatalogIntent::Startup,
            || async {
                port.record_catalog();
                Ok(())
            },
        )
        .await;

        let error =
            result.expect_err("normal startup must require completed pre-start maintenance");
        assert!(error.to_string().contains("pre-start maintenance command"));
        assert!(!port.events().contains(&"catalog"));
    }

    #[tokio::test]
    async fn partial_catalog_failure_keeps_maintenance_claim_ambiguous() {
        let port = FakePort::with_preexisting_database();

        let result = run_v4_catalog_maintenance_with_intent(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            V4CatalogIntent::PreStartMaintenance,
            || async {
                port.record_catalog();
                Err(MemoryError::Storage(
                    "catalog stopped after partial DDL".into(),
                ))
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            port.claim().expect("claim remains durable").state,
            ClaimState::Ambiguous,
            "a catalog can have applied some DDL before returning an error"
        );
    }

    #[test]
    fn maintenance_attempt_id_contains_a_host_specific_nonce() {
        let first = super::new_attempt_id();
        let second = super::new_attempt_id();
        let nonce = first.rsplit('-').next().expect("nonce suffix");

        assert!(first.starts_with(&format!("{}-", std::process::id())));
        assert_eq!(nonce.len(), 16);
        assert!(nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[tokio::test]
    async fn provenance_is_recorded_once_and_conflicts_fail_closed() {
        let port = FakePort::new("4.0.1", None);
        port.state
            .lock()
            .expect("fake port mutex")
            .database_is_empty = true;

        let compatibility = establish_database_provenance(&port, DatabaseProvenance::FreshV4)
            .await
            .expect("fresh v4 provenance is recorded");
        assert_eq!(compatibility, super::ServerCompatibility::V4Maintenance);
        assert_eq!(
            port.state.lock().expect("fake port mutex").provenance,
            Some(DatabaseProvenance::FreshV4)
        );

        let error = establish_database_provenance(&port, DatabaseProvenance::PreExisting)
            .await
            .expect_err("provenance cannot be overwritten by a conflicting startup classification");
        assert!(error.to_string().contains("conflicts"));
    }

    #[tokio::test]
    async fn fresh_v4_provenance_is_not_recorded_for_a_nonempty_database() {
        let port = FakePort::new("4.0.1", None);

        let error = establish_database_provenance(&port, DatabaseProvenance::FreshV4)
            .await
            .expect_err("a nonempty database cannot be classified as fresh v4");

        assert!(error.to_string().contains("not empty"));
        assert_eq!(port.state.lock().expect("fake port mutex").provenance, None);
    }

    #[tokio::test]
    async fn stable_surrealdb_3_skips_maintenance_without_provenance() {
        let port = FakePort::new("3.3.5", None);

        let outcome = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect("stable SurrealDB 3.x uses the compatible path");

        assert_eq!(outcome, MaintenanceOutcome::SkippedLegacyCompatible);
        assert_eq!(port.rebuild_calls(), 0);
        assert!(port.claim().is_none());
    }

    #[tokio::test]
    async fn preexisting_v4_database_rebuilds_each_managed_index_before_completion() {
        let port = FakePort::with_preexisting_database();
        let indexes = managed_indexes_for_role(DatabaseRole::PrimaryMemory);

        let outcome = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect("pre-existing v4 database maintenance completes");

        assert_eq!(outcome, MaintenanceOutcome::Completed);
        assert_eq!(port.rebuild_calls(), indexes.len());
        let claim = port.claim().expect("durable completed claim");
        assert_eq!(claim.state, ClaimState::Completed);
        assert!(
            claim
                .indexes
                .values()
                .all(|state| *state == IndexProgress::Verified)
        );
        let saved = port.saved_records();
        assert!(saved.iter().any(|record| {
            record
                .indexes
                .values()
                .any(|state| *state == IndexProgress::Intent)
        }));
        assert!(saved.iter().any(|record| {
            record
                .indexes
                .values()
                .any(|state| *state == IndexProgress::Submitted)
        }));
    }

    #[tokio::test]
    async fn rebuild_response_loss_blocks_automatic_takeover_until_operator_confirmation() {
        let port = FakePort::with_preexisting_database();
        port.state.lock().expect("fake port mutex").fail_rebuild = true;

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("a lost rebuild response is ambiguous");
        assert!(error.to_string().contains("uncertain result"));
        assert_eq!(port.rebuild_calls(), 1);
        let claim = port.claim().expect("ambiguous claim is durable");
        assert_eq!(claim.state, ClaimState::Ambiguous);
        assert_eq!(
            claim.indexes.get("edge_temporal_idx"),
            Some(&IndexProgress::Ambiguous)
        );

        port.set_fail_rebuild(false);
        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("ordinary startup cannot take over an ambiguous claim");
        assert!(error.to_string().contains("incomplete"));
        assert_eq!(port.rebuild_calls(), 1);

        let outcome = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume,
        )
        .await
        .expect("explicitly authorized recovery retries the uncertain index");
        assert_eq!(outcome, MaintenanceOutcome::Completed);
        assert_eq!(
            port.rebuild_calls(),
            1 + managed_indexes_for_role(DatabaseRole::PrimaryMemory).len()
        );
    }

    #[tokio::test]
    async fn post_rebuild_inspection_error_is_ambiguous_not_a_failed_build() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .fail_post_rebuild_inspection = true;

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("a lost postcondition query cannot prove index failure");

        assert!(error.to_string().contains("inspection failed"));
        let claim = port.claim().expect("ambiguous claim remains durable");
        assert_eq!(claim.state, ClaimState::Ambiguous);
        assert_eq!(
            claim.indexes.get("edge_temporal_idx"),
            Some(&IndexProgress::Ambiguous)
        );
        assert_eq!(port.rebuild_calls(), 1);
    }

    #[tokio::test]
    async fn unknown_index_build_state_blocks_maintenance_before_claim() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .unknown_build_state = true;

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("unknown build state must fail closed");

        assert!(
            error
                .to_string()
                .contains("unknown server-side build state")
        );
        assert!(port.claim().is_none());
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn running_server_side_build_blocks_claim_and_new_rebuilds() {
        let port = FakePort::with_preexisting_database();
        port.state.lock().expect("fake port mutex").running_build = true;

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("active server work must block maintenance");

        assert!(error.to_string().contains("active server-side build"));
        assert!(port.claim().is_none());
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn death_before_submission_requires_operator_authorized_recovery() {
        let port = FakePort::with_preexisting_database();
        let key = MaintenanceKey {
            namespace: "test_namespace".to_owned(),
            database: "test_database".to_owned(),
            revision: MAINTENANCE_REVISION,
        };
        let mut indexes = BTreeMap::new();
        indexes.insert("edge_temporal_idx".to_owned(), IndexProgress::Intent);
        port.state.lock().expect("fake port mutex").claim = Some(MaintenanceRecord {
            key: MaintenanceKeyRecord::from(&key),
            role: "primary_memory".to_owned(),
            provenance: DatabaseProvenance::PreExisting,
            server_version: "4.0.1".to_owned(),
            owner_id: "old-owner".to_owned(),
            attempt_id: "old-attempt".to_owned(),
            state: ClaimState::Active,
            current_index: Some("edge_temporal_idx".to_owned()),
            indexes,
            updated_at_ms: 1,
        });

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("a claim left before submission must not be taken over automatically");
        assert!(error.to_string().contains("active in another process"));
        assert_eq!(port.rebuild_calls(), 0);

        run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume,
        )
        .await
        .expect("operator-authorized recovery proceeds");
        assert_eq!(
            port.rebuild_calls(),
            managed_indexes_for_role(DatabaseRole::PrimaryMemory).len()
        );
    }

    #[tokio::test]
    async fn failed_completion_write_keeps_normal_startup_blocked() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .fail_completion_write = true;

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("completion write failure cannot report readiness");
        assert!(error.to_string().contains("could not be durably recorded"));
        assert_eq!(
            port.rebuild_calls(),
            managed_indexes_for_role(DatabaseRole::PrimaryMemory).len()
        );
        assert_ne!(
            port.claim().expect("claim remains").state,
            ClaimState::Completed
        );

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("the incomplete claim blocks ordinary startup");
        assert!(error.to_string().contains("active in another process"));
    }

    #[tokio::test]
    async fn failed_post_rebuild_definition_check_is_recorded_as_failed() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .fail_definition_on_post_catalog_inspection = Some("edge_temporal_idx");

        let error = run_test_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            RecoveryAuthorization::None,
        )
        .await
        .expect_err("a changed managed-index definition must stop the rebuild");

        assert!(error.to_string().contains("does not match"));
        let claim = port.claim().expect("failed claim remains durable");
        assert_eq!(claim.state, ClaimState::Failed);
        assert_eq!(
            claim.indexes.get("edge_temporal_idx"),
            Some(&IndexProgress::Failed)
        );
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn unparseable_server_versions_fail_closed() {
        let error = parse_server_version("version unavailable")
            .expect_err("an unparseable server version must fail closed");

        assert!(error.to_string().contains("version unavailable"));
    }

    #[tokio::test]
    async fn v4_maintenance_refuses_a_database_without_recorded_provenance() {
        let port = FakePort::new("4.0.1", None);

        let error = run_v4_catalog_maintenance_with_intent(
            &port,
            DatabaseRole::Registry,
            false,
            false,
            RecoveryAuthorization::None,
            V4CatalogIntent::Startup,
            || async { Ok(()) },
        )
        .await
        .expect_err("missing provenance must block v4 startup");

        assert!(error.to_string().contains("provenance"));
    }

    #[test]
    fn stable_surrealdb_3_uses_the_compatible_path() {
        let version = ServerVersion::new(3, false, "3.3.5");

        assert_eq!(
            classify_server_version(&version).unwrap(),
            ServerCompatibility::LegacyCompatible
        );
    }

    #[test]
    fn stable_surrealdb_4_requires_v4_maintenance() {
        let version = ServerVersion::new(4, false, "4.0.1");

        assert_eq!(
            classify_server_version(&version).unwrap(),
            ServerCompatibility::V4Maintenance
        );
    }

    #[test]
    fn prerelease_and_future_major_versions_fail_closed() {
        let prerelease = ServerVersion::new(4, true, "4.1.0-beta.1");
        let future_major = ServerVersion::new(5, false, "5.0.0");

        assert!(classify_server_version(&prerelease).is_err());
        assert!(classify_server_version(&future_major).is_err());
    }

    #[tokio::test]
    async fn schema_catalog_runs_under_an_exclusive_claim_before_index_rebuilds() {
        let port = FakePort::with_preexisting_database();

        let outcome = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                port.record_catalog();
                Ok(())
            },
        )
        .await
        .expect("catalog and v4 maintenance should complete");

        assert_eq!(outcome, MaintenanceOutcome::Completed);
        assert_eq!(port.events().first(), Some(&"claim"));
        assert_eq!(port.events().get(1), Some(&"catalog"));
        assert_eq!(port.events().get(2), Some(&"rebuild"));
        assert_eq!(
            port.claim().expect("durable claim").state,
            ClaimState::Completed
        );
    }

    #[tokio::test]
    async fn active_server_build_blocks_catalog_before_claim_or_mutation() {
        let port = FakePort::with_preexisting_database();
        port.state.lock().expect("fake port mutex").running_build = true;
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("an existing server-side build must block catalog mutation");

        assert!(error.to_string().contains("active server-side build"));
        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn server_build_starting_after_preflight_blocks_catalog_mutation() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .start_build_when_claimed = true;
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("a build that starts after preflight must block the catalog");

        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(error.to_string().contains("active server-side build"));
        assert_eq!(
            port.claim().expect("claim remains durable").state,
            ClaimState::Failed
        );
    }

    #[tokio::test]
    async fn unknown_server_build_state_blocks_catalog_before_claim_or_mutation() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .unknown_build_state = true;
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("an unknown server-side build state must fail closed");

        assert!(
            error
                .to_string()
                .contains("unknown server-side build state")
        );
        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn competing_startup_reports_active_maintenance_without_suggesting_takeover() {
        let port = FakePort::with_preexisting_database();
        let identity = DatabaseIdentity {
            namespace: "test_namespace".into(),
            database: "test_database".into(),
        };
        let key = MaintenanceKey::new(identity);
        let version = parse_server_version("4.0.1").expect("valid test version");
        let claim = super::new_claim(
            &key,
            DatabaseRole::HttpTenant,
            DatabaseProvenance::PreExisting,
            &version,
            managed_indexes_for_role(DatabaseRole::HttpTenant),
        );
        port.state.lock().expect("fake port mutex").claim = Some(claim);
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("a competing startup must not take the active claim");

        assert!(error.to_string().contains("active in another process"));
        assert!(!error.to_string().contains("recover-ambiguous"));
        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn partial_catalog_failure_requires_explicit_ambiguous_recovery() {
        let port = FakePort::with_preexisting_database();

        let first = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Err(MemoryError::Storage("catalog interrupted".into())) },
        )
        .await;
        assert!(
            first.is_err(),
            "the interrupted catalog remains unavailable"
        );
        assert_eq!(
            port.claim().expect("durable claim").state,
            ClaimState::Ambiguous
        );

        let ordinary_retry = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Ok(()) },
        )
        .await;
        assert!(
            ordinary_retry.is_err(),
            "startup cannot take over ambiguous DDL"
        );

        let retry = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume,
            || async { Ok(()) },
        )
        .await;

        assert!(retry.is_ok(), "explicit recovery may retry the catalog");
        assert_eq!(
            port.claim().expect("durable claim").state,
            ClaimState::Completed
        );
    }

    #[tokio::test]
    async fn corrected_definition_allows_retry_after_a_known_pre_rebuild_failure() {
        let port = FakePort::with_preexisting_database();
        port.state
            .lock()
            .expect("fake port mutex")
            .fail_definition_on_post_catalog_inspection = Some("edge_temporal_idx");

        let first = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Ok(()) },
        )
        .await;
        assert!(
            first.is_err(),
            "the bad definition must fail the first attempt"
        );

        let retry = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Ok(()) },
        )
        .await;

        assert!(retry.is_ok(), "a now-ready definition is safe to retry");
        assert_eq!(
            port.claim().expect("durable claim").state,
            ClaimState::Completed
        );
    }

    #[tokio::test]
    async fn completed_claim_allows_catalog_to_restore_a_missing_managed_index() {
        let port = FakePort::with_preexisting_database();
        run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Ok(()) },
        )
        .await
        .expect("initial catalog and maintenance");

        port.hide_index_until_catalog("fact_t_valid");
        let outcome = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async {
                port.record_catalog();
                Ok(())
            },
        )
        .await
        .expect("the catalog should restore the missing index before verification");

        assert_eq!(outcome, MaintenanceOutcome::Completed);
        assert_eq!(
            port.claim().expect("completed claim").state,
            ClaimState::Completed
        );
    }

    #[tokio::test]
    async fn completed_claim_does_not_bypass_remote_catalog_acknowledgement() {
        let port = FakePort::with_preexisting_database();
        run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            false,
            false,
            RecoveryAuthorization::None,
            || async { Ok(()) },
        )
        .await
        .expect("initial catalog and maintenance");
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::HttpTenant,
            true,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("a completed local claim must not authorize remote schema changes");

        assert!(
            error
                .to_string()
                .contains("SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD")
        );
        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn remote_v4_catalog_is_refused_before_claim_or_catalog_without_acknowledgement() {
        let port = FakePort::with_preexisting_database();
        let catalog_ran = std::sync::atomic::AtomicBool::new(false);

        let error = run_v4_catalog_maintenance(
            &port,
            DatabaseRole::PrimaryMemory,
            true,
            false,
            RecoveryAuthorization::None,
            || async {
                catalog_ran.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("remote v4 mutation requires operator acknowledgement");

        assert!(
            error
                .to_string()
                .contains("SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD")
        );
        assert!(!catalog_ran.load(std::sync::atomic::Ordering::SeqCst));
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
    }

    #[tokio::test]
    async fn stable_surrealdb_3_preflight_allows_the_legacy_catalog_without_v4_mutations() {
        let port = FakePort::new("3.3.5", None);

        let compatibility = preflight_v4_catalog(&port, DatabaseRole::HttpTenant, true, false)
            .await
            .expect("stable SurrealDB 3 is compatible without v4 maintenance");

        assert_eq!(compatibility, ServerCompatibility::LegacyCompatible);
        assert!(port.events().is_empty());
    }

    #[tokio::test]
    async fn remote_v4_preflight_refuses_before_any_durable_write_without_acknowledgement() {
        let port = FakePort::with_preexisting_database();

        let error = preflight_v4_catalog(&port, DatabaseRole::HttpTenant, true, false)
            .await
            .expect_err("remote v4 requires the maintenance-window acknowledgement");

        assert!(
            error
                .to_string()
                .contains("SURREALDB_ALLOW_REMOTE_V4_INDEX_REBUILD")
        );
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
    }

    #[tokio::test]
    async fn v4_preflight_rejects_unknown_provenance_without_mutating_the_database() {
        let port = FakePort::new("4.0.1", None);

        let error = preflight_v4_catalog(&port, DatabaseRole::PrimaryMemory, false, false)
            .await
            .expect_err("nonempty v4 databases need durable provenance before catalog changes");

        assert!(error.to_string().contains("migration provenance"));
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
    }

    #[tokio::test]
    async fn prestart_preflight_rejects_active_build_before_any_catalog_write() {
        let port = FakePort::with_preexisting_database();
        port.state.lock().expect("fake port mutex").running_build = true;

        let error = preflight_v4_catalog_with_intent(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            V4CatalogIntent::PreStartMaintenance,
        )
        .await
        .expect_err("preflight must reject an active managed index build");

        assert!(error.to_string().contains("active server-side build"));
        assert!(port.events().is_empty());
        assert_eq!(port.rebuild_calls(), 0);
    }

    #[tokio::test]
    async fn prestart_preflight_rejects_claim_for_another_role_before_catalog_write() {
        let port = FakePort::with_preexisting_database();
        let key = MaintenanceKey {
            namespace: "test_namespace".to_owned(),
            database: "test_database".to_owned(),
            revision: MAINTENANCE_REVISION,
        };
        port.state.lock().expect("fake port mutex").claim = Some(MaintenanceRecord {
            key: MaintenanceKeyRecord::from(&key),
            role: "registry".to_owned(),
            provenance: DatabaseProvenance::PreExisting,
            server_version: "4.0.1".to_owned(),
            owner_id: "old-owner".to_owned(),
            attempt_id: "old-attempt".to_owned(),
            state: ClaimState::Ambiguous,
            current_index: None,
            indexes: BTreeMap::new(),
            updated_at_ms: 1,
        });

        let error = super::preflight_v4_catalog_with_recovery(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            V4CatalogIntent::PreStartMaintenance,
            RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume,
        )
        .await
        .expect_err("operator recovery must not bypass a claim belonging to another role");

        assert!(error.to_string().contains("different role or provenance"));
        assert!(port.events().is_empty());
        assert_eq!(
            port.claim().expect("claim remains").attempt_id,
            "old-attempt"
        );
    }

    #[tokio::test]
    async fn explicit_prestart_preflight_reads_unknown_provenance_without_mutating() {
        let port = FakePort::new("4.0.1", None);

        let compatibility = preflight_v4_catalog_with_intent(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            V4CatalogIntent::PreStartMaintenance,
        )
        .await
        .expect("explicit pre-start maintenance may inspect an existing database");

        assert_eq!(compatibility, ServerCompatibility::V4Maintenance);
        assert_eq!(port.preflight_reads(), (1, 1));
        assert!(port.events().is_empty());
        assert!(port.claim().is_none());
    }

    #[tokio::test]
    async fn explicit_prestart_maintenance_marks_an_unrecorded_nonempty_v4_as_preexisting() {
        let port = FakePort::new("4.0.1", None);

        let outcome = super::run_v4_catalog_maintenance_with_intent(
            &port,
            DatabaseRole::PrimaryMemory,
            false,
            false,
            RecoveryAuthorization::None,
            super::V4CatalogIntent::PreStartMaintenance,
            || async {
                port.record_catalog();
                Ok(())
            },
        )
        .await
        .expect("explicit pre-start intent admits an unrecorded existing database");

        assert_eq!(outcome, MaintenanceOutcome::Completed);
        assert_eq!(
            port.state.lock().expect("fake port mutex").provenance,
            Some(DatabaseProvenance::PreExisting)
        );
        assert_eq!(port.events().first(), Some(&"claim"));
        assert_eq!(port.events().get(1), Some(&"catalog"));
        assert_eq!(
            port.rebuild_calls(),
            managed_indexes_for_role(DatabaseRole::PrimaryMemory).len()
        );
        assert_eq!(
            port.claim().map(|claim| claim.state),
            Some(ClaimState::Completed)
        );
    }

    #[test]
    fn the_memory_manifest_contains_only_the_audited_rebuild_indexes() {
        let expected = [
            "edge_temporal_idx",
            "fact_t_valid",
            "episode_t_ref",
            "edge_t_valid",
            "claim_relation_left_active_idx",
            "claim_relation_right_active_idx",
            "claim_job_lease_idx",
            "inbox_revision_state_lease",
            "entity_aliases",
            "community_members",
            "community_summary_search",
            "fact_embedding_hnsw",
        ];

        for role in [DatabaseRole::PrimaryMemory, DatabaseRole::HttpTenant] {
            let actual: Vec<_> = managed_indexes_for_role(role)
                .iter()
                .map(|index| index.name)
                .collect();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn maintenance_revision_advances_when_the_rebuild_manifest_changes() {
        assert_eq!(MAINTENANCE_REVISION, "surrealdb_4_index_rebuild_v2");
    }

    #[test]
    fn registry_datetime_index_is_audited_but_not_rebuilt() {
        let index = registry_audit_index();

        assert_eq!(index.name, "idx_audit_event_account");
        assert_eq!(index.table, "audit_event");
        assert!(managed_indexes_for_role(DatabaseRole::Registry).is_empty());
    }
}
