#[path = "common/storage_memory.rs"]
#[allow(dead_code)]
mod storage_memory;

use std::error::Error;

use storage_memory::StorageMemoryFixture;

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
