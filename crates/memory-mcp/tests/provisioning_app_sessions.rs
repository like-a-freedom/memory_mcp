#![cfg(feature = "mcp-apps")]

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use memory_mcp::MemoryError;
use memory_mcp::provisioning::api::{
    AppSessionCommand, AppSessionPersistence, AppSessionRecord, AppSessionUsage,
    OpenAppSessionCommand, close_app_session, open_app_session, read_app_session,
    write_app_session,
};

struct FakeStore {
    record: Mutex<Option<AppSessionRecord>>,
    cap: u32,
}

impl FakeStore {
    fn session() -> Self {
        Self {
            record: Mutex::new(None),
            cap: 32,
        }
    }
}

#[async_trait::async_trait]
impl AppSessionPersistence for FakeStore {
    async fn open(
        &self,
        app: &str,
        payload: serde_json::Value,
        max_open_per_tenant: u32,
        _now: DateTime<Utc>,
    ) -> Result<AppSessionRecord, MemoryError> {
        assert!(max_open_per_tenant <= self.cap);
        let record = AppSessionRecord {
            handle: "sess_1".into(),
            app: app.into(),
            version: 1,
            payload,
            idle_expiry: Utc::now() + Duration::minutes(30),
            absolute_expiry: Utc::now() + Duration::hours(24),
        };
        *self.record.lock().expect("record lock") = Some(record.clone());
        Ok(record)
    }

    async fn load(&self, _handle: &str) -> Result<Option<AppSessionRecord>, MemoryError> {
        Ok(self.record.lock().expect("record lock").clone())
    }

    async fn command(
        &self,
        _handle: &str,
        expected_version: u64,
        payload: serde_json::Value,
    ) -> Result<u64, MemoryError> {
        let mut guard = self.record.lock().expect("record lock");
        let record = guard.as_mut().expect("open first");
        if record.version != expected_version {
            return Err(MemoryError::Conflict("app_session version conflict".into()));
        }
        record.payload = payload;
        record.version += 1;
        Ok(record.version)
    }

    async fn close(&self, _handle: &str) -> Result<(), MemoryError> {
        *self.record.lock().expect("record lock") = None;
        Ok(())
    }
}

#[tokio::test]
async fn app_session_lifecycle_is_versioned_and_expiry_bound() {
    let store = Arc::new(FakeStore::session());
    let opened = open_app_session(
        store.as_ref(),
        OpenAppSessionCommand {
            app: "ingestion_review".into(),
            payload: serde_json::json!({"items": []}),
            max_open_per_tenant: 32,
        },
    )
    .await
    .expect("open");
    assert_eq!(opened.handle, "sess_1");
    assert_eq!(opened.version, 1);

    let view = read_app_session(
        store.as_ref(),
        &AppSessionCommand {
            handle: opened.handle.clone(),
        },
        Utc::now(),
    )
    .await
    .expect("read")
    .expect("live session");
    assert!(view.expires_at > Utc::now());

    let updated = write_app_session(
        store.as_ref(),
        &AppSessionCommand {
            handle: opened.handle.clone(),
        },
        &AppSessionUsage {
            expected_version: 1,
            payload: serde_json::json!({"items": ["a"]}),
        },
    )
    .await
    .expect("update");
    assert_eq!(updated, 2);

    assert!(
        write_app_session(
            store.as_ref(),
            &AppSessionCommand {
                handle: opened.handle.clone(),
            },
            &AppSessionUsage {
                expected_version: 1,
                payload: serde_json::json!({"items": ["stale"]}),
            },
        )
        .await
        .is_err(),
        "a stale version must be refused"
    );

    close_app_session(
        store.as_ref(),
        &AppSessionCommand {
            handle: opened.handle.clone(),
        },
    )
    .await
    .expect("close");
    assert!(
        read_app_session(
            store.as_ref(),
            &AppSessionCommand {
                handle: opened.handle,
            },
            Utc::now(),
        )
        .await
        .expect("read after close")
        .is_none()
    );
}

#[tokio::test]
async fn app_session_view_rejects_idle_expired_sessions() {
    let store = Arc::new(FakeStore::session());
    let mut record = store
        .open("graph", serde_json::json!({}), 32, Utc::now())
        .await
        .expect("open");
    record.idle_expiry = Utc::now() - Duration::minutes(1);
    *store.record.lock().expect("record lock") = Some(record);

    assert!(
        read_app_session(
            store.as_ref(),
            &AppSessionCommand {
                handle: "sess_1".into()
            },
            Utc::now(),
        )
        .await
        .expect("read expired")
        .is_none(),
        "an idle-expired session is not readable"
    );
}
