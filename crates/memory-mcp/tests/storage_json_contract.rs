#[path = "common/storage_memory.rs"]
#[allow(dead_code)]
mod storage_memory;

use std::error::Error;

use memory_mcp::storage::DbClient;
use memory_mcp::storage::table_scope::{KnowledgeTables, ReleaseOwnedTable};
use storage_memory::StorageMemoryFixture;
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;
use surrealdb::types::{Number, Value as SurrealValue};

const TYPED_VALUE_QUERY: &str = r#"
RETURN {
    uuid: u"00000000-0000-4000-8000-000000000001",
    bytes: b"007fff",
    duration: 1h30m,
    point: (1.25, -2.5),
    table: type::table("entity"),
    regex: <regex> "a.*b",
    range: 1..3,
    set: {"alpha", "beta"},
    file: f"fixture:/sample.txt",
    record_ids: [
        type::record("fact", "trip-a"),
        type::record("fact", "key with space"),
        type::record("fact", "ключ"),
        type::record("fact", "a:b"),
        type::record("fact", 7),
        type::record("fact", [1, "a"])
    ]
};
"#;

#[tokio::test(flavor = "current_thread")]
async fn storage_query_preserves_typed_value_json_contract() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let actual = fixture.query_json(TYPED_VALUE_QUERY).await?;
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/storage_json/typed-values-v1.json"))?;

    assert_eq!(actual, expected);

    Ok(())
}

async fn query_result(sql: &str) -> Result<serde_json::Value, Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    fixture.query_json(sql).await
}

async fn raw_surreal_client(
    fixture: &StorageMemoryFixture,
) -> Result<Surreal<Client>, Box<dyn Error>> {
    let db = Surreal::new::<Ws>(fixture.url.as_str()).await?;
    db.signin(Root {
        username: "root".to_string(),
        password: "root".to_string(),
    })
    .await?;
    db.use_ns(&fixture.namespace)
        .use_db(&fixture.database)
        .await?;
    Ok(db)
}

async fn seed_payload_from_expression(
    fixture: &StorageMemoryFixture,
    record_key: &str,
    expression: &str,
) -> Result<(), Box<dyn Error>> {
    let db = raw_surreal_client(fixture).await?;
    let mut response = db.query(format!("RETURN {expression};")).await?;
    let payload = response.take::<SurrealValue>(0)?;
    db.query(format!(
        "CREATE ONLY conversion_fixture:{record_key} SET payload = $payload;"
    ))
    .bind(("payload", payload))
    .await?
    .check()?;
    Ok(())
}

async fn select_one_record(
    fixture: &StorageMemoryFixture,
    record_id: &str,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let client = fixture.connect_adapter().await?;
    client
        .select_one(record_id, &fixture.namespace)
        .await?
        .ok_or_else(|| "expected the fixture record to exist".into())
}

#[tokio::test(flavor = "current_thread")]
async fn query_none_has_no_rows() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN NONE;").await?;

    assert_eq!(actual, serde_json::json!([]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_explicit_null_has_one_null_row() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN NULL;").await?;

    assert_eq!(actual, serde_json::json!([null]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn nested_nullable_fields_are_json_null() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN { a: NONE, b: NULL };").await?;

    assert_eq!(actual, serde_json::json!([{"a": null, "b": null}]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn bool_scalar_is_json_bool() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN true;").await?;

    assert_eq!(actual, serde_json::json!([true]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_preserves_float_samples_exactly() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let expected = serde_json::json!([{
        "samples": [0.0, 1.0_f64 / 17.0, 16.0_f64 / 17.0]
    }]);
    let actual = fixture
        .query_json_with_vars(
            "RETURN { samples: [$zero, $one, $sixteen] };",
            Some(serde_json::json!({
                "zero": 0.0,
                "one": 1.0_f64 / 17.0,
                "sixteen": 16.0_f64 / 17.0,
            })),
        )
        .await?;

    assert_eq!(actual, expected);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_preserves_large_decimal_precision_as_a_string() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN { decimal: 12345678901234567890.123456789dec };").await?;

    assert_eq!(
        actual,
        serde_json::json!([{
            "decimal": "12345678901234567890.123456789"
        }])
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn query_preserves_datetime_fractional_seconds() -> Result<(), Box<dyn Error>> {
    let actual = query_result("RETURN { timestamp: d'2026-01-01T00:00:00.123456Z' };").await?;

    assert_eq!(
        actual,
        serde_json::json!([{
            "timestamp": "2026-01-01T00:00:00.123456Z"
        }])
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn select_one_preserves_raw_sdk_typed_value_json() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    seed_payload_from_expression(
        &fixture,
        "rare_types",
        r#"{
            uuid: u"00000000-0000-4000-8000-000000000001",
            bytes: b"007fff",
            duration: 1h30m,
            point: (1.25, -2.5),
            table: type::table("entity"),
            regex: <regex> "a.*b",
            range: 1..3,
            set: {"alpha", "beta"},
            file: f"fixture:/sample.txt",
            record_ids: [
                type::record("fact", "trip-a"),
                type::record("fact", "key with space"),
                type::record("fact", "ключ"),
                type::record("fact", "a:b"),
                type::record("fact", 7),
                type::record("fact", [1, "a"])
            ]
        }"#,
    )
    .await?;
    let actual = select_one_record(&fixture, "conversion_fixture:rare_types").await?;
    let expected_rows: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/storage_json/typed-values-v1.json"))?;
    let expected = expected_rows
        .as_array()
        .and_then(|rows| rows.first())
        .ok_or("the typed-values literal fixture must contain one row")?;

    assert_eq!(actual.get("payload"), Some(expected));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn select_one_returns_a_plain_fact_record() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let actual = select_one_record(&fixture, "fact:allocation_probe_0000").await?;

    assert_eq!(
        actual.get("fact_id"),
        Some(&serde_json::json!("fact:allocation_probe_0000"))
    );
    assert_eq!(
        actual.get("content"),
        Some(&serde_json::json!("synthetic allocation probe 0"))
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn missing_record_is_none() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let client = fixture.connect_adapter().await?;
    let actual = client
        .select_one("fact:missing_record", &fixture.namespace)
        .await?;

    assert_eq!(actual, None);
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn select_table_returns_plain_fact_records() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let client = fixture.connect_adapter().await?;
    let facts = client
        .select_table(KnowledgeTables::table("fact"), &fixture.namespace)
        .await?;

    assert_eq!(facts.len(), 1);
    assert_eq!(
        facts[0].get("fact_id"),
        Some(&serde_json::json!("fact:allocation_probe_0000"))
    );
    assert_eq!(
        facts[0]
            .get("embedding")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(17)
    );
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn create_returns_a_plain_json_record() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let client = fixture.connect_adapter().await?;
    let content = serde_json::json!({"enabled": true, "ratio": 1.0_f64 / 17.0});
    let actual = client
        .create(
            "conversion_fixture:created",
            content.clone(),
            &fixture.namespace,
            &[],
        )
        .await?;

    assert_eq!(actual.get("enabled"), content.get("enabled"));
    assert_eq!(actual.get("ratio"), content.get("ratio"));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn update_returns_a_plain_json_record() -> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let client = fixture.connect_adapter().await?;
    client
        .create(
            "conversion_fixture:updated",
            serde_json::json!({"state": "before"}),
            &fixture.namespace,
            &[],
        )
        .await?;
    let actual = client
        .update(
            "conversion_fixture:updated",
            serde_json::json!({"state": "after", "active": true}),
            &fixture.namespace,
            &[],
        )
        .await?;

    assert_eq!(actual.get("state"), Some(&serde_json::json!("after")));
    assert_eq!(actual.get("active"), Some(&serde_json::json!(true)));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn server_version_returns_none_when_database_info_has_no_version()
-> Result<(), Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let client = fixture.connect_adapter().await?;
    let info = fixture.query_json("INFO FOR DB;").await?;
    let info_fields = info
        .as_array()
        .and_then(|results| results.first())
        .and_then(serde_json::Value::as_object)
        .ok_or("INFO FOR DB must return one object")?;

    assert!(
        !info_fields
            .keys()
            .any(|field| field.to_ascii_lowercase().contains("version")),
        "INFO FOR DB unexpectedly exposed a version field: {info_fields:?}"
    );
    let version = client.server_version(&fixture.namespace).await?;

    assert_eq!(version, None);
    Ok(())
}

async fn select_one_non_finite_number(
    record_key: &str,
    number: f64,
) -> Result<serde_json::Value, Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(1, 17).await?;
    let db = raw_surreal_client(&fixture).await?;
    db.query(format!(
        "CREATE ONLY conversion_fixture:{record_key} SET value = $value;"
    ))
    .bind(("value", SurrealValue::Number(Number::Float(number))))
    .await?
    .check()?;

    select_one_record(&fixture, &format!("conversion_fixture:{record_key}")).await
}

#[tokio::test(flavor = "current_thread")]
async fn non_finite_nan_is_json_null() -> Result<(), Box<dyn Error>> {
    let actual = select_one_non_finite_number("nan", f64::NAN).await?;

    assert_eq!(actual.get("value"), Some(&serde_json::Value::Null));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn non_finite_positive_infinity_is_json_null() -> Result<(), Box<dyn Error>> {
    let actual = select_one_non_finite_number("positive_infinity", f64::INFINITY).await?;

    assert_eq!(actual.get("value"), Some(&serde_json::Value::Null));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn non_finite_negative_infinity_is_json_null() -> Result<(), Box<dyn Error>> {
    let actual = select_one_non_finite_number("negative_infinity", f64::NEG_INFINITY).await?;

    assert_eq!(actual.get("value"), Some(&serde_json::Value::Null));
    Ok(())
}
