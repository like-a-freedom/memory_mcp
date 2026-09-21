#![cfg(all(feature = "streamable-http", feature = "control-plane"))]

//! CLI coverage for the administrator-only `admin` command (plan Task 5).
//!
//! The parser tests pin the clap surface. The process test spawns the real
//! `memory_mcp` binary against an isolated, file-backed RocksDB control
//! registry: it proves `admin create` / `admin recover` persist an
//! administrator across processes and never require tenant credentials,
//! model settings or OIDC configuration (those variables are deliberately
//! absent from the child environment).
//!
//! Run:
//! `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test local_admin_cli`

use std::collections::BTreeSet;
use std::process::Command as ProcessCommand;

use clap::Parser;
use memory_mcp::cli::args::{AdminOperation, AuthMethodsArgs, AuthMethodsOperation};
use memory_mcp::cli::{Cli, Command};

const SESSION_KEY_HEX: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const CSRF_KEY_HEX: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const PUBLIC_BASE_URL: &str = "https://admin.example.test";

/// Parse an `admin` argv and return the parsed `Command`.
fn admin_command(args: &[&str]) -> Command {
    let mut argv = vec!["memory_mcp", "admin"];
    argv.extend_from_slice(args);
    let cli = Cli::try_parse_from(argv).expect("admin command should parse");
    match cli.command {
        Some(command @ Command::Admin(_)) => command,
        other => panic!("expected admin command, got {other:?}"),
    }
}

#[test]
fn admin_create_parses_typed_operation() {
    let command = admin_command(&["create", "--username", "ops.one"]);
    let Command::Admin(args) = command else {
        panic!("expected admin command");
    };
    assert_eq!(
        args.operation,
        AdminOperation::Create {
            username: "ops.one".to_string(),
        }
    );
}

#[test]
fn admin_recover_parses_typed_operation() {
    let command = admin_command(&["recover", "--username", "ops.one"]);
    let Command::Admin(args) = command else {
        panic!("expected admin command");
    };
    assert_eq!(
        args.operation,
        AdminOperation::Recover {
            username: "ops.one".to_string(),
        }
    );
}

#[test]
fn admin_subcommands_reject_non_admin_flags() {
    // Client-management, password and one-time-code flags belong to the HTTP
    // surface, never to the administrator CLI. Each must fail as an unknown
    // argument rather than being silently accepted.
    let cases: &[&[&str]] = &[
        &["create", "--username", "x", "--password", "y"],
        &["create", "--username", "x", "--code", "y"],
        &["create", "--username", "x", "--display-name", "y"],
        &["create", "--username", "x", "--client", "y"],
        &["create", "--username", "x", "--secret", "y"],
        &["recover", "--username", "x", "--password", "y"],
        &["recover", "--username", "x", "--code", "y"],
    ];

    for args in cases {
        let mut argv = vec!["memory_mcp", "admin"];
        argv.extend_from_slice(args);
        let error = Cli::try_parse_from(argv).expect_err("non-admin flag must be rejected");
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "unexpected parse error for {args:?}: {error}"
        );
    }
}

#[test]
fn admin_command_is_not_a_one_shot_and_has_stable_mode_label() {
    let command = admin_command(&["create", "--username", "ops.one"]);
    assert_eq!(command.mode_label(), "cli.admin");
    assert!(command.into_one_shot().is_none());
}

#[test]
fn admin_variant_is_reachable_in_this_build() {
    // This integration target only compiles when `control-plane` (which
    // enables `streamable-http`) is on, so the *negative* case — that a
    // default-features build does not expose `Command::Admin` — cannot be
    // observed from inside this build. That case is covered by the
    // documented help check instead:
    //
    //   cargo run -p memory_mcp --no-default-features --locked --bin memory_mcp -- --help
    //
    // which must not list `admin`. Here we assert the positive reachability.
    let command = admin_command(&["recover", "--username", "ops.one"]);
    assert!(matches!(command, Command::Admin(_)));
}

/// Spawn the real binary with only administrator configuration present and
/// return its captured output.
fn run_admin(dir: &tempfile::TempDir, args: &[&str]) -> std::process::Output {
    run_admin_with_mode(dir, args, Some("local"))
}

/// As [`run_admin`], but with an explicit
/// `MEMORY_MCP_HTTP_AUTH_METHODS` (or none at all) so the method requirement
/// can be exercised.
fn run_admin_with_mode(
    dir: &tempfile::TempDir,
    args: &[&str],
    methods: Option<&str>,
) -> std::process::Output {
    run_admin_with_operators(dir, args, methods, None)
}

/// Spawn the admin CLI against an isolated control registry, optionally with a
/// method set and an operator allowlist in the child environment.
fn run_admin_with_operators(
    dir: &tempfile::TempDir,
    args: &[&str],
    methods: Option<&str>,
    operator_identities: Option<&str>,
) -> std::process::Output {
    let url = format!("rocksdb://{}/db", dir.path().display());
    let mut command = ProcessCommand::new(env!("CARGO_BIN_EXE_memory_mcp"));
    command
        .env_clear()
        .env("RUST_LOG", "error")
        .env("SURREALDB_CONTROL_URL", url)
        .env("SURREALDB_CONTROL_USERNAME", "root")
        .env("SURREALDB_CONTROL_PASSWORD", "root")
        .env("SURREALDB_CONTROL_DB", "registry")
        .env("SURREALDB_CONTROL_NAMESPACE", "local_admin_cli")
        .env("MEMORY_MCP_HTTP_SESSION_KEY", SESSION_KEY_HEX)
        .env("MEMORY_MCP_HTTP_CSRF_KEY", CSRF_KEY_HEX)
        .env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", PUBLIC_BASE_URL);
    if let Some(methods) = methods {
        command.env("MEMORY_MCP_HTTP_AUTH_METHODS", methods);
    }
    if let Some(operators) = operator_identities {
        command.env("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES", operators);
    }
    command
        .args(args)
        .output()
        .expect("spawn memory_mcp admin subprocess")
}

/// Parse stdout as exactly one JSON object, failing on anything else.
fn single_json_object(stdout: &str) -> serde_json::Value {
    let values = serde_json::Deserializer::from_str(stdout)
        .into_iter::<serde_json::Value>()
        .collect::<Result<Vec<_>, _>>()
        .expect("stdout must be a stream of JSON values");
    assert_eq!(
        values.len(),
        1,
        "stdout must contain exactly one JSON object: {stdout:?}"
    );
    assert!(
        values[0].is_object(),
        "stdout JSON must be an object: {stdout:?}"
    );
    values.into_iter().next().expect("one value")
}

fn object_keys(value: &serde_json::Value) -> BTreeSet<&str> {
    value
        .as_object()
        .expect("JSON object")
        .keys()
        .map(String::as_str)
        .collect()
}

/// Assert that a successful admin run neither initialized a model nor
/// attempted OIDC discovery. Both failures would surface as error lines.
fn assert_no_oidc_or_model_markers(stderr_lowercase: &str) {
    for marker in ["oidc", "discovery", "modelnotready", "model_not_ready"] {
        assert!(
            !stderr_lowercase.contains(marker),
            "stderr must not mention `{marker}`: {stderr_lowercase}"
        );
    }
}

#[test]
fn admin_create_then_recover_persists_across_processes() {
    let dir = tempfile::tempdir().expect("tempdir");

    // First process: create the administrator.
    let create = run_admin(&dir, &["admin", "create", "--username", "ops.one"]);
    assert!(
        create.status.success(),
        "admin create must succeed with only admin configuration: status={:?} stderr={}",
        create.status,
        String::from_utf8_lossy(&create.stderr)
    );
    let create_stdout = String::from_utf8(create.stdout).expect("utf8 stdout");
    let create_json = single_json_object(&create_stdout);
    assert_eq!(
        object_keys(&create_json),
        [
            "admin_id",
            "username",
            "code",
            "expires_at",
            "activation_url"
        ]
        .into_iter()
        .collect::<BTreeSet<&str>>()
    );
    assert_eq!(create_json["username"], "ops.one");
    let activation_code = create_json["code"]
        .as_str()
        .expect("activation code")
        .to_string();
    assert!(
        !activation_code.is_empty(),
        "activation code must be issued"
    );
    let activation_url = create_json["activation_url"]
        .as_str()
        .expect("activation url");
    assert!(
        activation_url.ends_with("/admin/activate"),
        "activation URL must be the fixed activate route: {activation_url}"
    );
    assert!(
        !activation_url.contains(&activation_code),
        "activation URL must not embed the one-time code"
    );

    let create_stderr = String::from_utf8_lossy(&create.stderr).to_lowercase();
    assert!(
        !create_stderr.contains(&activation_code),
        "secrets belong on stdout only; stderr must not contain the code"
    );
    assert_no_oidc_or_model_markers(&create_stderr);

    // Second process against the same RocksDB path: `recover` only succeeds
    // when the administrator committed by the first process is durable.
    let recover = run_admin(&dir, &["admin", "recover", "--username", "ops.one"]);
    assert!(
        recover.status.success(),
        "admin recover must succeed against the persisted administrator: status={:?} stderr={}",
        recover.status,
        String::from_utf8_lossy(&recover.stderr)
    );
    let recover_stdout = String::from_utf8(recover.stdout).expect("utf8 stdout");
    let recover_json = single_json_object(&recover_stdout);
    assert_eq!(
        object_keys(&recover_json),
        ["admin_id", "username", "code", "expires_at", "reset_url"]
            .into_iter()
            .collect::<BTreeSet<&str>>()
    );
    assert_eq!(recover_json["username"], "ops.one");
    let reset_code = recover_json["code"]
        .as_str()
        .expect("reset code")
        .to_string();
    assert_ne!(
        activation_code, reset_code,
        "recovery must issue fresh one-time material"
    );
    let reset_url = recover_json["reset_url"].as_str().expect("reset url");
    assert!(
        reset_url.ends_with("/admin/reset"),
        "reset URL must be the fixed reset route: {reset_url}"
    );
    assert!(
        !reset_url.contains(&reset_code),
        "reset URL must not embed the one-time code"
    );

    let recover_stderr = String::from_utf8_lossy(&recover.stderr).to_lowercase();
    assert!(
        !recover_stderr.contains(&reset_code),
        "secrets belong on stdout only; stderr must not contain the code"
    );
    assert_no_oidc_or_model_markers(&recover_stderr);
}

/// Spec §6: the admin commands **require the local method**. Running one in a
/// deployment that authenticates browsers through an identity provider alone
/// (or that names an unknown method) must fail before any registry is opened or
/// written.
#[test]
fn admin_commands_require_the_local_method() {
    for (methods, expected) in [
        (None, "require the 'local' browser authentication method"),
        (
            Some("oidc"),
            "require the 'local' browser authentication method",
        ),
        (Some("saml"), "must be 'local' or 'oidc'"),
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let output =
            run_admin_with_mode(&dir, &["admin", "create", "--username", "ops.one"], methods);
        assert!(
            !output.status.success(),
            "methods {methods:?} must be refused, got {:?}",
            output.status
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "methods {methods:?} must name the requirement, got: {stderr}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            !stdout.contains("\"code\""),
            "no activation code may be issued with methods {methods:?}: {stdout}"
        );
    }
}

#[test]
fn admin_auth_methods_remove_parses_typed_operation() {
    let command = admin_command(&["auth-methods", "remove", "--method", "local"]);
    let Command::Admin(args) = command else {
        panic!("expected admin command");
    };
    assert_eq!(
        args.operation,
        AdminOperation::AuthMethods(AuthMethodsArgs {
            operation: AuthMethodsOperation::Remove {
                method: "local".to_string(),
            },
        })
    );
}

#[test]
fn admin_auth_methods_remove_requires_a_method_argument() {
    assert!(
        Cli::try_parse_from(["memory_mcp", "admin", "auth-methods", "remove"]).is_err(),
        "--method is required"
    );
    assert!(
        Cli::try_parse_from(["memory_mcp", "admin", "auth-methods", "remove", "--method"]).is_err(),
        "a missing --method value must not parse"
    );
}

/// ADR-0057: the removal is guarded, and every refusal happens before the
/// registry is opened — so a refused command neither creates nor writes one.
#[test]
fn admin_auth_method_removal_is_guarded_before_the_registry_opens() {
    for (methods, operators, expected) in [
        (
            Some("local"),
            None,
            "still enabled by MEMORY_MCP_HTTP_AUTH_METHODS",
        ),
        (
            Some("local,oidc"),
            Some("https://idp.example|ab"),
            "still enabled by MEMORY_MCP_HTTP_AUTH_METHODS",
        ),
        (Some("oidc"), None, "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES"),
    ] {
        let dir = tempfile::tempdir().expect("temp dir");
        let output = run_admin_with_operators(
            &dir,
            &["admin", "auth-methods", "remove", "--method", "local"],
            methods,
            operators,
        );
        assert!(
            !output.status.success(),
            "methods {methods:?} / operators {operators:?} must be refused, got {:?}",
            output.status
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(expected),
            "methods {methods:?} / operators {operators:?} must name the refusal reason, \
             got: {stderr}"
        );
        assert!(
            !dir.path().join("db").exists(),
            "a refused removal must not create a registry"
        );
    }
}

/// The end-to-end path to "SSO only": an administrator is created (which
/// reconciles a `local,oidc` policy), then the local method is removed, and the
/// configuration the command reports is the one a restart would need.
#[test]
fn admin_auth_methods_remove_reaches_sso_only() {
    let dir = tempfile::tempdir().expect("temp dir");

    let created = run_admin_with_mode(
        &dir,
        &["admin", "create", "--username", "ops.one"],
        Some("local,oidc"),
    );
    assert!(
        created.status.success(),
        "admin create must reconcile a local,oidc policy: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let removed = run_admin_with_operators(
        &dir,
        &["admin", "auth-methods", "remove", "--method", "local"],
        Some("oidc"),
        Some("https://idp.example|ab"),
    );
    assert!(
        removed.status.success(),
        "removing the local method must succeed: {}",
        String::from_utf8_lossy(&removed.stderr)
    );
    let stdout = String::from_utf8_lossy(&removed.stdout);
    let report = single_json_object(&stdout);
    assert_eq!(report["removed_method"], "local");
    assert_eq!(report["enabled_methods"], "oidc");
    assert_eq!(report["epoch"], "2", "the removal advances the epoch");
    let guidance = report["guidance"].as_str().expect("guidance");
    assert!(
        guidance.contains("MEMORY_MCP_HTTP_AUTH_METHODS=oidc"),
        "the command must hand the operator the configuration to deploy: {guidance}"
    );

    // The method is gone: a second removal is refused by the store, not by the
    // guard, because the configuration no longer names it either.
    let again = run_admin_with_operators(
        &dir,
        &["admin", "auth-methods", "remove", "--method", "local"],
        Some("oidc"),
        Some("https://idp.example|ab"),
    );
    assert!(!again.status.success(), "a method cannot be removed twice");
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert!(
        stderr.contains("is not enabled by the durable policy"),
        "the store must report the absent method, got: {stderr}"
    );
}
