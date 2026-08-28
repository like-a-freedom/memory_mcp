// Placeholder binary stub. The Phase 3 Task 3.10 composition root replaces
// this entire file with the real environment-driven HTTP composition root.
// The current body is only here so `cargo build --features streamable-http`
// resolves the [[bin]] target declared in Cargo.toml before that task lands.
fn main() {
    eprintln!(
        "memory_mcp_http: composition root not implemented yet (Phase 3 / Task 3.10)"
    );
    std::process::exit(1);
}
