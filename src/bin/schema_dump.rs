use schemars::schema_for;

fn main() {
    let schema = schema_for!(memory_mcp::mcp::AssembleContextParams);
    let json = serde_json::to_string_pretty(&schema).unwrap();
    println!("{json}");
}
