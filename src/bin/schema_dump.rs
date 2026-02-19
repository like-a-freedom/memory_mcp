use schemars::schema_for;
use std::process;

fn main() {
    let schema = schema_for!(memory_mcp::mcp::AssembleContextParams);
    match serde_json::to_string_pretty(&schema) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("Failed to serialize schema: {e}");
            process::exit(1);
        }
    }
}
