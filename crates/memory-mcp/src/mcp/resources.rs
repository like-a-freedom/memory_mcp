//! Resource URI helpers and catalog entries for MCP Apps.
#![cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]

use rmcp::model::{Resource, ResourceTemplate};
use serde_json::json;

pub(crate) const APPS_INDEX_URI: &str = "ui://memory/apps";
const APPS_ROOT_PREFIX: &str = "ui://memory/apps/";
const APP_SESSION_PREFIX: &str = "ui://memory/app/";

const PUBLIC_APPS: [(&str, &str); 5] = [
    (
        "inspector",
        "Inspect one memory object with temporal state and provenance.",
    ),
    ("diff", "Compare memory state between two timestamps."),
    (
        "ingestion_review",
        "Review extracted draft items before commit.",
    ),
    (
        "lifecycle",
        "Inspect lifecycle operations and hygiene workflows.",
    ),
    ("graph", "Explore graph paths and neighborhood context."),
];

#[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
pub(crate) fn app_catalog_resources() -> Vec<Resource> {
    #[cfg(feature = "mcp-apps")]
    {
        let mut resources = vec![
            Resource::new(APPS_INDEX_URI, "Memory Apps")
                .with_description("Catalog of public MCP Apps and their resource contracts.")
                .with_mime_type("application/json"),
        ];

        resources.extend(PUBLIC_APPS.into_iter().map(|(app, description)| {
            Resource::new(app_root_uri(app), format!("Memory App: {app}"))
                .with_description(description)
                .with_mime_type("application/json")
        }));

        resources
    }

    #[cfg(not(feature = "mcp-apps"))]
    {
        Vec::new()
    }
}

#[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
pub(crate) fn app_resource_templates() -> Vec<ResourceTemplate> {
    #[cfg(feature = "mcp-apps")]
    {
        PUBLIC_APPS
            .into_iter()
            .map(|(app, description)| {
                ResourceTemplate::new(
                    app_session_uri_template(app),
                    format!("Memory App Session: {app}"),
                )
                .with_description(format!(
                    "{description} Open a session with `open_app`, then read the concrete session URI or use this template for discovery."
                ))
                .with_mime_type("text/html;profile=mcp-app")
            })
            .collect()
    }

    #[cfg(not(feature = "mcp-apps"))]
    {
        Vec::new()
    }
}

pub(crate) fn app_root_uri(app: &str) -> String {
    format!("{APPS_ROOT_PREFIX}{app}")
}

pub(crate) fn app_session_uri_template(app: &str) -> String {
    format!("{APP_SESSION_PREFIX}{app}/{{session_id}}")
}

pub(crate) fn app_session_uri(app: &str, session_id: &str) -> String {
    format!("{APP_SESSION_PREFIX}{app}/{session_id}")
}

pub(crate) fn parse_app_root_uri(uri: &str) -> Option<String> {
    uri.strip_prefix(APPS_ROOT_PREFIX)
        .filter(|rest| !rest.is_empty() && !rest.contains('/'))
        .map(ToOwned::to_owned)
}

pub(crate) fn parse_app_session_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix(APP_SESSION_PREFIX)?;
    let (app, session_id) = rest.split_once('/')?;
    if app.is_empty() || session_id.is_empty() {
        return None;
    }

    Some((app.to_string(), session_id.to_string()))
}

#[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
pub(crate) fn apps_index_payload() -> serde_json::Value {
    #[cfg(feature = "mcp-apps")]
    {
        json!({
            "apps": PUBLIC_APPS
                .into_iter()
                .map(|(app, description)| {
                    json!({
                        "app": app,
                        "description": description,
                        "root_resource_uri": app_root_uri(app),
                        "session_resource_template": app_session_uri_template(app),
                    })
                })
                .collect::<Vec<_>>()
        })
    }

    #[cfg(not(feature = "mcp-apps"))]
    {
        json!({ "apps": [] })
    }
}

#[cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]
pub(crate) fn app_root_payload(app: &str) -> Option<serde_json::Value> {
    #[cfg(feature = "mcp-apps")]
    {
        let description = PUBLIC_APPS
            .into_iter()
            .find(|(candidate, _)| *candidate == app)
            .map(|(_, description)| description)?;

        Some(json!({
            "app": app,
            "description": description,
            "session_resource_template": app_session_uri_template(app),
        }))
    }

    #[cfg(not(feature = "mcp-apps"))]
    {
        let _ = app;
        None
    }
}

pub(crate) fn app_session_html_document(app: &str, payload: &serde_json::Value) -> String {
    let title = format!("Memory App: {app}");
    let json_payload = serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string());

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
    <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; media-src 'self' data:; frame-src 'none'; object-src 'none'; base-uri 'self';" />
        <title>{title}</title>
        <style>
            :root {{ color-scheme: light dark; }}
            body {{ margin: 0; font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: Canvas; color: CanvasText; }}
            main {{ padding: 1rem; max-width: 72rem; margin: 0 auto; }}
            pre {{ white-space: pre-wrap; word-break: break-word; background: color-mix(in srgb, CanvasText 8%, Canvas 92%); padding: 1rem; border-radius: 12px; overflow: auto; }}
            .badge {{ display: inline-block; padding: 0.25rem 0.5rem; border-radius: 999px; background: color-mix(in srgb, CanvasText 12%, Canvas 88%); font-size: 0.8rem; }}
        </style>
    </head>
    <body>
        <main>
            <span class="badge">MCP App</span>
            <h1>{title}</h1>
            <p>This is the session resource for the {app} app. Compliant hosts render this HTML inline.</p>
            <script type="application/json" id="app-data">{json_payload}</script>
            <pre id="app-preview"></pre>
            <script>
                const data = JSON.parse(document.getElementById('app-data').textContent || '{{}}');
                document.getElementById('app-preview').textContent = JSON.stringify(data, null, 2);
            </script>
        </main>
    </body>
</html>"#,
        title = title,
        app = app,
        json_payload = json_payload,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_session_uri_round_trips() {
        let uri = app_session_uri("inspector", "ses:test-123");
        assert_eq!(uri, "ui://memory/app/inspector/ses:test-123");
        assert_eq!(
            parse_app_session_uri(&uri),
            Some(("inspector".to_string(), "ses:test-123".to_string()))
        );
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_catalog_contains_root_resource_and_all_public_apps() {
        let resources = app_catalog_resources();
        let uris: Vec<_> = resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect();

        assert!(uris.contains(&APPS_INDEX_URI));
        assert!(uris.contains(&"ui://memory/apps/inspector"));
        assert!(uris.contains(&"ui://memory/apps/diff"));
        assert!(uris.contains(&"ui://memory/apps/ingestion_review"));
        assert!(uris.contains(&"ui://memory/apps/lifecycle"));
        assert!(uris.contains(&"ui://memory/apps/graph"));
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_resource_templates_expose_session_templates_for_all_public_apps() {
        let templates = app_resource_templates();
        let uris: Vec<_> = templates
            .iter()
            .map(|template| template.uri_template.as_str())
            .collect();
        let mime_types: Vec<_> = templates
            .iter()
            .map(|template| template.mime_type.as_deref())
            .collect();

        assert!(uris.contains(&"ui://memory/app/inspector/{session_id}"));
        assert!(uris.contains(&"ui://memory/app/diff/{session_id}"));
        assert!(uris.contains(&"ui://memory/app/ingestion_review/{session_id}"));
        assert!(uris.contains(&"ui://memory/app/lifecycle/{session_id}"));
        assert!(uris.contains(&"ui://memory/app/graph/{session_id}"));
        assert!(
            mime_types
                .iter()
                .all(|mime_type| *mime_type == Some("text/html;profile=mcp-app"))
        );
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_session_html_document_wraps_payload_for_app_shell() {
        let html = app_session_html_document("inspector", &json!({"app": "inspector"}));

        assert!(html.contains("Memory App: inspector"));
        assert!(html.contains("<script type=\"application/json\" id=\"app-data\">"));
        assert!(html.contains("\"app\": \"inspector\""));
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_root_uri_returns_none_for_index_uri() {
        assert!(parse_app_root_uri(APPS_INDEX_URI).is_none());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_root_uri_returns_none_for_empty_suffix() {
        assert!(parse_app_root_uri(APPS_ROOT_PREFIX).is_none());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_root_uri_returns_none_for_nested_path() {
        assert!(parse_app_root_uri("ui://memory/apps/inspector/sub").is_none());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_root_uri_extracts_app_name() {
        assert_eq!(
            parse_app_root_uri("ui://memory/apps/inspector"),
            Some("inspector".to_string())
        );
        assert_eq!(
            parse_app_root_uri("ui://memory/apps/diff"),
            Some("diff".to_string())
        );
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_session_uri_returns_none_for_empty_parts() {
        assert!(parse_app_session_uri("ui://memory/app//ses:123").is_none());
        assert!(parse_app_session_uri("ui://memory/app/inspector/").is_none());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn parse_app_session_uri_returns_none_for_missing_prefix() {
        assert!(parse_app_session_uri("ui://memory/apps/inspector/ses:123").is_none());
    }

    #[test]
    fn app_root_payload_returns_none_for_unknown_app() {
        assert!(app_root_payload("nonexistent").is_none());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_root_payload_returns_app_info_for_known_app() {
        let payload = app_root_payload("inspector");
        assert!(payload.is_some());
        let payload = payload.unwrap();
        assert_eq!(payload["app"], "inspector");
        assert!(payload["description"].as_str().is_some());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn apps_index_payload_contains_all_public_apps() {
        let payload = apps_index_payload();
        let apps = payload["apps"].as_array().expect("apps should be array");
        assert_eq!(apps.len(), PUBLIC_APPS.len());
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_session_html_document_handles_complex_payload() {
        let payload = json!({
            "app": "diff",
            "timestamps": ["2024-01-01", "2024-01-02"],
            "metadata": {"key": "value"}
        });
        let html = app_session_html_document("diff", &payload);
        assert!(html.contains("Memory App: diff"));
        assert!(html.contains("\"timestamps\""));
        assert!(html.contains("\"metadata\""));
    }

    #[cfg(feature = "mcp-apps")]
    #[test]
    fn app_uri_helpers_consistent() {
        let app = "lifecycle";
        let root = app_root_uri(app);
        let template = app_session_uri_template(app);
        let session = app_session_uri(app, "abc123");

        assert_eq!(root, "ui://memory/apps/lifecycle");
        assert_eq!(template, "ui://memory/app/lifecycle/{session_id}");
        assert_eq!(session, "ui://memory/app/lifecycle/abc123");
    }
}
