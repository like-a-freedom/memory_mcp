use eval_harness::EvalError;
use eval_harness::paired::{CaseTrialKey, RunArm};
use eval_harness::paired_agent::chat_completions::MemoryToolSession;
use eval_harness::paired_agent::public_trace::{PublicSyntheticSession, SyntheticTraceIdentity};
use serde_json::{Value, json};

struct Memory {
    calls: Vec<Value>,
    schemas: Value,
}
impl MemoryToolSession for Memory {
    fn limit_deadline(&mut self, _: std::time::Instant) {}

    fn tool_schemas(&self) -> &Value {
        &self.schemas
    }
    fn call_tool(&mut self, _: &str, arguments: Value) -> Result<Value, EvalError> {
        self.calls.push(arguments);
        Ok(json!({"result":{"structuredContent":{"result":[]}}}))
    }
}

fn identity() -> SyntheticTraceIdentity {
    SyntheticTraceIdentity {
        plan_sha256: "a".repeat(64),
        product_binary_sha256: "b".repeat(64),
        tool_schema_sha256: "4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945"
            .into(),
        key: CaseTrialKey {
            stratum_id: "public".into(),
            family_id: "correction".into(),
            case_id: "public-correction".into(),
            repetition: 0,
            incumbent_first: true,
        },
        arm: RunArm::Candidate,
        phase: "recall".into(),
    }
}

#[test]
fn synthetic_capture_replays_exact_public_arguments_and_binds_response_identity() {
    let identity = identity();
    let mut memory = Memory {
        calls: vec![],
        schemas: json!([]),
    };
    let mut captured = PublicSyntheticSession::new(&mut memory, identity.clone()).unwrap();
    let arguments =
        json!({"query":"Which plan?", "as_of":null, "budget":5, "entity_ids":["entity:public"]});
    captured
        .call_tool("assemble_context", arguments.clone())
        .unwrap();
    let trace = captured.finish();
    let serialized = serde_json::to_value(&trace).unwrap();
    assert_eq!(serialized["calls"][0]["arguments"], arguments);
    let mut replay = Memory {
        calls: vec![],
        schemas: json!([]),
    };
    trace.replay_for(&identity, &mut replay).unwrap();
    assert_eq!(replay.calls, vec![arguments]);
    let mut tampered = serialized;
    tampered["calls"][0]["response"] =
        json!({"result":{"structuredContent":{"result":["forged"]}}});
    let changed = serde_json::from_value::<
        eval_harness::paired_agent::public_trace::PublicSyntheticTrace,
    >(tampered)
    .unwrap();
    assert!(changed.replay_for(&identity, &mut replay).is_err());
    assert_eq!(
        replay.calls.len(),
        1,
        "validate all evidence before executing any replay"
    );
}

#[test]
fn a_trace_from_a_different_registered_arm_cannot_be_replayed() {
    let mut memory = Memory {
        calls: vec![],
        schemas: json!([]),
    };
    let trace = PublicSyntheticSession::new(&mut memory, identity())
        .unwrap()
        .finish();
    let mut expected = identity();
    expected.arm = RunArm::Incumbent;
    assert!(trace.replay_for(&expected, &mut memory).is_err());
    assert!(memory.calls.is_empty());
}
