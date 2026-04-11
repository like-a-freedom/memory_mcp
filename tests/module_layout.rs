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

#[test]
fn entity_extraction_subsystem_lives_under_a_single_directory_module() {
    let service_dir = service_dir();
    let service_mod = fs::read_to_string(service_dir.join("mod.rs"))
        .expect("service root module should be readable");
    let entity_extraction_mod = fs::read_to_string(service_dir.join("entity_extraction/mod.rs"))
        .expect("entity_extraction mod.rs should be readable");

    assert!(
        service_mod.contains("\nmod entity_extraction;"),
        "service root should keep entity_extraction as a directory module"
    );
    assert!(
        !service_mod.contains("\nmod anno_entity_extractor;"),
        "service root should not keep anno provider as a separate top-level module"
    );
    assert!(
        !service_mod.contains("\nmod gliner_entity_extractor;"),
        "service root should not keep gliner provider as a separate top-level module"
    );
    assert!(
        !service_mod.contains("pub use anno_entity_extractor::AnnoEntityExtractor;"),
        "service root should re-export anno extractor via entity_extraction"
    );
    assert!(
        !service_mod.contains("pub use gliner_entity_extractor::GlinerEntityExtractor;"),
        "service root should re-export gliner extractor via entity_extraction"
    );

    assert!(
        entity_extraction_mod.contains("\nmod anno;"),
        "entity_extraction mod.rs should declare the anno provider submodule"
    );
    assert!(
        entity_extraction_mod.contains("\nmod gliner;"),
        "entity_extraction mod.rs should declare the gliner provider submodule"
    );
    assert!(
        entity_extraction_mod.contains("\nmod regex;"),
        "entity_extraction mod.rs should declare the regex provider submodule"
    );
    assert!(
        entity_extraction_mod.contains("\nmod classifier;"),
        "entity_extraction mod.rs should declare the shared classifier submodule"
    );

    assert!(
        !service_dir.join("entity_extraction.rs").exists(),
        "legacy src/service/entity_extraction.rs should be removed after the physical move"
    );
    assert!(
        !service_dir.join("anno_entity_extractor.rs").exists(),
        "legacy src/service/anno_entity_extractor.rs should be removed after grouping extractors"
    );
    assert!(
        !service_dir.join("gliner_entity_extractor.rs").exists(),
        "legacy src/service/gliner_entity_extractor.rs should be removed after grouping extractors"
    );
}
