#![cfg(feature = "control-plane")]

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use memory_mcp::provisioning::api::{
    ClientAuthority, ClientCreation, ClientCreationError, ClientCreationPort, ClientView,
    CreateClientCommand, create_client,
};

#[derive(Default)]
struct RecordingClientCreation {
    commands: Mutex<Vec<ClientCreation>>,
}

#[async_trait::async_trait]
impl ClientCreationPort for RecordingClientCreation {
    async fn create_client(
        &self,
        command: ClientCreation,
    ) -> Result<ClientView, ClientCreationError> {
        self.commands
            .lock()
            .expect("commands lock")
            .push(command.clone());
        Ok(ClientView {
            account_id: command.account_id,
            tenant_id: command.tenant_id,
            display_name: command.display_name,
            account_status: "active".into(),
            tenant_status: "reserved".into(),
            plan_version: command.plan_version,
            schema_version: command.schema_version,
            version: 0,
            provisioning_reason: None,
        })
    }
}

fn at(timestamp: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(timestamp)
        .expect("fixed timestamp")
        .with_timezone(&Utc)
}

#[tokio::test]
async fn client_creation_forwards_the_fixed_authority_and_idempotency_fingerprint() {
    let now = at("2026-10-03T12:00:00Z");
    let operation_id =
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").expect("operation id");
    let request_id =
        uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").expect("request id");
    let port = RecordingClientCreation::default();
    let authority = ClientAuthority {
        admin_id: "admin_1".into(),
        session_verifier: "session_verifier_1".into(),
        credential_generation: 4,
        policy_epoch: 3,
        policy_methods: vec![
            memory_mcp::identity::api::AuthMethod::Local,
            memory_mcp::identity::api::AuthMethod::Oidc,
        ],
        request_id,
    };

    let result = create_client(
        &port,
        CreateClientCommand {
            authority: authority.clone(),
            operation_id,
            display_name: "team-alpha".into(),
            plan_version: 2,
        },
        now,
    )
    .await
    .expect("create client");

    let commands = port.commands.lock().expect("commands lock");
    assert_eq!(commands.len(), 1);
    let command = &commands[0];
    assert_eq!(command.authority, authority);
    assert_eq!(command.operation_id, operation_id);
    assert_eq!(command.display_name, "team-alpha");
    assert_eq!(command.plan_version, 2);
    assert_eq!(command.database, "memory");
    assert_eq!(command.schema_version, 0);
    assert_eq!(command.now, now);
    assert_eq!(
        command.request_fingerprint,
        [
            0x6c, 0x8f, 0xf3, 0x28, 0xa2, 0xbf, 0x88, 0x78, 0x3b, 0x96, 0x56, 0x42, 0x11, 0xf8,
            0xce, 0xa8, 0xd9, 0x4c, 0x2e, 0xe7, 0x3d, 0xf7, 0xcb, 0xd2, 0x3b, 0xfd, 0x1b, 0xd2,
            0xcb, 0x14, 0xb9, 0xe4,
        ]
    );
    assert_eq!(result.account_id, command.account_id);
    assert_eq!(result.tenant_id, command.tenant_id);
    assert_eq!(result.display_name, "team-alpha");
}
