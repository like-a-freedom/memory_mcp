use std::fs;
use std::path::Path;

fn service_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service")
}

#[test]
fn service_root_uses_default_module_resolution_for_directory_modules() {
    let service_mod = fs::read_to_string(service_dir().join("mod.rs"))
        .expect("service root module should be readable");

    assert!(
        service_mod.contains("\nmod core;"),
        "service root should declare `mod core;` without a path override"
    );
    assert!(
        service_mod.contains("\nmod episode;"),
        "service root should declare `mod episode;` without a path override"
    );
    assert!(
        !service_mod.contains("#[path = \"core/mod.rs\"]"),
        "service root should not use an explicit path override for core"
    );
    assert!(
        !service_mod.contains("#[path = \"episode/mod.rs\"]"),
        "service root should not use an explicit path override for episode"
    );
}

#[test]
fn core_module_lives_directly_in_directory_mod_rs() {
    let service_dir = service_dir();
    let core_mod = fs::read_to_string(service_dir.join("core/mod.rs"))
        .expect("core mod.rs should be readable");

    assert!(
        !core_mod.contains("include!(\"../core.rs\")"),
        "core mod.rs should contain the real implementation, not an include wrapper"
    );
    assert!(
        !service_dir.join("core.rs").exists(),
        "legacy src/service/core.rs should be removed after the physical move"
    );
}

#[test]
fn episode_module_lives_directly_in_directory_mod_rs() {
    let service_dir = service_dir();
    let episode_mod = fs::read_to_string(service_dir.join("episode/mod.rs"))
        .expect("episode mod.rs should be readable");

    assert!(
        !episode_mod.contains("include!(\"../episode.rs\")"),
        "episode mod.rs should contain the real implementation, not an include wrapper"
    );
    assert!(
        !service_dir.join("episode.rs").exists(),
        "legacy src/service/episode.rs should be removed after the physical move"
    );
}
