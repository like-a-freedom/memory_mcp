use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

type TreeSnapshot = BTreeMap<String, Vec<u8>>;

fn snapshot_tree(root: &Path) -> TreeSnapshot {
    fn visit(root: &Path, path: &Path, snapshot: &mut TreeSnapshot) {
        let metadata = fs::symlink_metadata(path).expect("read snapshot metadata");
        let relative = path
            .strip_prefix(root)
            .expect("snapshot path must be under root");
        let key = relative.to_string_lossy().into_owned();

        if metadata.is_dir() {
            if !key.is_empty() {
                snapshot.insert(format!("{key}/"), Vec::new());
            }
            for entry in fs::read_dir(path).expect("read snapshot directory") {
                visit(root, &entry.expect("read snapshot entry").path(), snapshot);
            }
        } else {
            snapshot.insert(key, fs::read(path).expect("read snapshot file"));
        }
    }

    let mut snapshot = TreeSnapshot::new();
    visit(root, root, &mut snapshot);
    snapshot
}

#[test]
fn init_targets_are_json_and_non_mutating() {
    let binary = env!("CARGO_BIN_EXE_memory_mcp");
    let targets = ["vscode", "claude-desktop", "codex", "zed", "env"];

    for target in targets {
        let test_dir = TempDir::new().expect("temporary init test directory");
        let home = test_dir.path().join("home");
        let xdg_data_home = test_dir.path().join("xdg");
        let current_dir = test_dir.path().join("cwd");
        fs::create_dir_all(&home).expect("create isolated HOME");
        fs::create_dir_all(&xdg_data_home).expect("create isolated XDG data directory");
        fs::create_dir_all(&current_dir).expect("create isolated current directory");
        fs::write(current_dir.join("sentinel.txt"), b"do not change").expect("create sentinel");

        let before = snapshot_tree(test_dir.path());
        let output = Command::new(binary)
            .env_clear()
            .env("HOME", &home)
            .env("XDG_DATA_HOME", &xdg_data_home)
            .current_dir(&current_dir)
            .args(["init", "--target", target])
            .output()
            .expect("run init command");

        assert!(
            output.status.success(),
            "init target {target} failed: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let result: Value = serde_json::from_slice(&output.stdout).expect("one JSON result");
        assert_eq!(result["target"], target);
        assert_eq!(result["mutates_files"], false);
        assert!(result["snippet"].as_str().is_some());
        assert_eq!(snapshot_tree(test_dir.path()), before);
        assert!(
            !xdg_data_home.join("memory_mcp").exists(),
            "init must not initialize the embedded data directory"
        );
    }
}
