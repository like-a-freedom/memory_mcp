//! Resource URI helpers and catalog entries for MCP Apps.

use rmcp::model::{AnnotateAble, RawResource, RawResourceTemplate, Resource, ResourceTemplate};
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

pub(crate) fn app_catalog_resources() -> Vec<Resource> {
    let mut resources = vec![
        RawResource::new(APPS_INDEX_URI, "Memory Apps")
            .with_description("Catalog of public MCP Apps and their resource contracts.")
            .with_mime_type("application/json")
            .no_annotation(),
    ];

    resources.extend(PUBLIC_APPS.into_iter().map(|(app, description)| {
        RawResource::new(app_root_uri(app), format!("Memory App: {app}"))
            .with_description(description)
            .with_mime_type("application/json")
            .no_annotation()
    }));

    resources
}

pub(crate) fn app_resource_templates() -> Vec<ResourceTemplate> {
    PUBLIC_APPS
        .into_iter()
        .map(|(app, description)| {
            RawResourceTemplate::new(
                app_session_uri_template(app),
                format!("Memory App Session: {app}"),
            )
            .with_description(format!(
                "{description} Open a session with `open_app`, then read the concrete session URI or use this template for discovery."
            ))
            .with_mime_type("text/html;profile=mcp-app")
            .no_annotation()
        })
        .collect()
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

pub(crate) fn apps_index_payload() -> serde_json::Value {
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

pub(crate) fn app_root_payload(app: &str) -> Option<serde_json::Value> {
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

    #[test]
    fn app_session_uri_round_trips() {
        let uri = app_session_uri("inspector", "ses:test-123");
        assert_eq!(uri, "ui://memory/app/inspector/ses:test-123");
        assert_eq!(
            parse_app_session_uri(&uri),
            Some(("inspector".to_string(), "ses:test-123".to_string()))
        );
    }

    #[test]
    fn app_catalog_contains_root_resource_and_all_public_apps() {
        let resources = app_catalog_resources();
        let uris: Vec<_> = resources
            .iter()
            .map(|resource| resource.raw.uri.as_str())
            .collect();

        assert!(uris.contains(&APPS_INDEX_URI));
        assert!(uris.contains(&"ui://memory/apps/inspector"));
        assert!(uris.contains(&"ui://memory/apps/diff"));
        assert!(uris.contains(&"ui://memory/apps/ingestion_review"));
        assert!(uris.contains(&"ui://memory/apps/lifecycle"));
        assert!(uris.contains(&"ui://memory/apps/graph"));
    }

    #[test]
    fn app_resource_templates_expose_session_templates_for_all_public_apps() {
        let templates = app_resource_templates();
        let uris: Vec<_> = templates
            .iter()
            .map(|template| template.raw.uri_template.as_str())
            .collect();
        let mime_types: Vec<_> = templates
            .iter()
            .map(|template| template.raw.mime_type.as_deref())
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
    #[test]
    fn app_session_html_document_wraps_payload_for_app_shell() {
        let html = app_session_html_document("inspector", &json!({"app": "inspector"}));

        assert!(html.contains("Memory App: inspector"));
        assert!(html.contains("<script type=\"application/json\" id=\"app-data\">"));
        assert!(html.contains("\"app\": \"inspector\""));
    }
}
