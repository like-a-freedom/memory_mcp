#![cfg(feature = "control-plane")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use memory_mcp::provisioning::api::{
    ClientAuthority, ClientCreation, ClientCreationError, ClientCreationPort, ClientView,
    CreateClientCommand, create_client,
};

type ClientCreationCall = (uuid::Uuid, String, [u8; 32], u32);

#[derive(Default)]
struct RecordingClientCreation {
    calls: Mutex<Vec<ClientCreationCall>>,
    by_operation: Mutex<HashMap<uuid::Uuid, (String, String)>>,
}

#[async_trait::async_trait]
impl ClientCreationPort for RecordingClientCreation {
    async fn create_client(
        &self,
        command: ClientCreation,
    ) -> Result<ClientView, ClientCreationError> {
        self.calls.lock().expect("calls lock").push((
            command.operation_id,
            command.display_name.clone(),
            command.request_fingerprint,
            command.plan_version,
        ));
        if let Some((account_id, tenant_id)) = self
            .by_operation
            .lock()
            .expect("operations lock")
            .get(&command.operation_id)
            .cloned()
        {
            return Ok(ClientView {
                account_id,
                tenant_id,
                display_name: command.display_name,
                account_status: "active".into(),
                tenant_status: "reserved".into(),
                plan_version: command.plan_version,
                schema_version: 0,
                version: 0,
                provisioning_reason: None,
            });
        }
        self.by_operation.lock().expect("operations lock").insert(
            command.operation_id,
            (command.account_id.clone(), command.tenant_id.clone()),
        );
        Ok(ClientView {
            account_id: command.account_id,
            tenant_id: command.tenant_id,
            display_name: command.display_name,
            account_status: "active".into(),
            tenant_status: "reserved".into(),
            plan_version: command.plan_version,
            schema_version: 0,
            version: 0,
            provisioning_reason: None,
        })
    }
}

#[tokio::test]
async fn client_creation_uses_one_atomic_command_and_deterministic_fingerprint() {
    let port = Arc::new(RecordingClientCreation::default());
    let operation_id = uuid::Uuid::new_v4();
    let command = CreateClientCommand {
        authority: ClientAuthority {
            admin_id: "admin_1".into(),
            session_verifier: "session_1".into(),
            credential_generation: 4,
            policy_epoch: 3,
            policy_methods: vec![
                memory_mcp::identity::api::AuthMethod::Local,
                memory_mcp::identity::api::AuthMethod::Oidc,
            ],
            request_id: uuid::Uuid::new_v4(),
        },
        operation_id,
        display_name: "team-alpha".into(),
        plan_version: 2,
    };

    let first = create_client(port.as_ref(), command.clone(), Utc::now())
        .await
        .expect("create client");
    let second = create_client(port.as_ref(), command, Utc::now())
        .await
        .expect("replay client");
    assert_eq!(first.account_id, second.account_id);
    assert_eq!(first.display_name, "team-alpha");

    let calls = port.calls.lock().expect("calls lock");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].0, calls[1].0);
    assert_eq!(calls[0].1, calls[1].1);
    assert_eq!(calls[0].2, calls[1].2);
}
