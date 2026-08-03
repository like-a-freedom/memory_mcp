use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;

use crate::logging::LogLevel;
use crate::models::IngestRequest;
use crate::service::{MemoryError, MemoryService, now};

pub const DEFAULT_WATCH_INTERVAL_SECS: u64 = 2;

pub struct FsWatcher;

impl FsWatcher {
    pub async fn run(
        dir: PathBuf,
        project: Option<String>,
        scope: String,
        service: MemoryService,
    ) -> Result<(), MemoryError> {
        Self::run_with_interval(dir, project, scope, DEFAULT_WATCH_INTERVAL_SECS, service).await
    }

    pub async fn run_with_interval(
        dir: PathBuf,
        project: Option<String>,
        scope: String,
        interval_secs: u64,
        service: MemoryService,
    ) -> Result<(), MemoryError> {
        if !dir.exists() || !dir.is_dir() {
            return Err(MemoryError::Validation(format!(
                "watch directory does not exist: {}",
                dir.display()
            )));
        }

        let effective_interval = Duration::from_secs(interval_secs.max(1));
        let (tx, rx) = mpsc::channel();
        let config = Config::default()
            .with_poll_interval(effective_interval)
            .with_follow_symlinks(false);
        let mut watcher = RecommendedWatcher::new(tx, config).map_err(|err| {
            MemoryError::Validation(format!(
                "failed to initialize filesystem watcher for {}: {err}",
                dir.display()
            ))
        })?;
        watcher
            .watch(&dir, RecursiveMode::Recursive)
            .map_err(|err| {
                MemoryError::Validation(format!(
                    "failed to watch directory {}: {err}",
                    dir.display()
                ))
            })?;

        service.logger.log(
            crate::service::log_event(
                "watcher.ready",
                json!({
                    "dir": dir.display().to_string(),
                    "scope": scope,
                    "project": project,
                    "interval_secs": interval_secs.max(1),
                }),
                json!({"status": "listening"}),
                None,
                None,
                None,
            ),
            LogLevel::Info,
        );

        let mut last_seen = HashMap::new();

        loop {
            let event = rx
                .recv()
                .map_err(|err| {
                    MemoryError::Validation(format!(
                        "watch channel closed for {}: {err}",
                        dir.display()
                    ))
                })?
                .map_err(|err| {
                    MemoryError::Validation(format!("watch error for {}: {err}", dir.display()))
                })?;

            for path in watched_paths_for_event(&event) {
                if !should_ingest_path(&path, &mut last_seen, Instant::now(), effective_interval) {
                    service.logger.log(
                        crate::service::log_event(
                            "watcher.event_skipped",
                            json!({"path": path.display().to_string()}),
                            json!({"reason": "interval_dedup"}),
                            None,
                            None,
                            None,
                        ),
                        LogLevel::Trace,
                    );
                    continue;
                }

                let source_type = source_type_for_path(&path).to_string();
                service.logger.log(
                    crate::service::log_event(
                        "watcher.ingest_dispatch",
                        json!({
                            "path": path.display().to_string(),
                            "source_type": source_type,
                            "scope": scope,
                            "project": project,
                        }),
                        json!({}),
                        None,
                        None,
                        None,
                    ),
                    LogLevel::Debug,
                );

                let ingest_result = crate::service::capabilities::ingest::IngestCapability::ingest(
                    &service.build_context(),
                    IngestRequest {
                        source_type: source_type.clone(),
                        source_id: format!("watch:{}", path.display()),
                        content: path.to_string_lossy().into_owned(),
                        t_ref: now(),
                        scope: scope.clone(),
                        project: project.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await;

                match ingest_result {
                    Ok(episode_id) => {
                        service.logger.log(
                            crate::service::log_event(
                                "watcher.ingest_complete",
                                json!({
                                    "path": path.display().to_string(),
                                    "source_type": source_type,
                                }),
                                json!({"episode_id": episode_id}),
                                None,
                                None,
                                None,
                            ),
                            LogLevel::Info,
                        );
                    }
                    Err(err) => {
                        service.logger.log(
                            crate::service::log_event(
                                "watcher.ingest_error",
                                json!({
                                    "path": path.display().to_string(),
                                    "source_type": source_type,
                                    "scope": scope,
                                    "project": project,
                                }),
                                json!({"error": err.to_string()}),
                                None,
                                None,
                                None,
                            ),
                            LogLevel::Error,
                        );
                        return Err(err);
                    }
                }
            }
        }
    }
}

fn watched_paths_for_event(event: &Event) -> Vec<PathBuf> {
    if !(event.kind.is_create() || event.kind.is_modify()) {
        return Vec::new();
    }

    event
        .paths
        .iter()
        .filter(|path| is_supported_watch_path(path))
        .cloned()
        .collect()
}

fn is_supported_watch_path(path: &Path) -> bool {
    if !path.exists() || !path.is_file() {
        return false;
    }

    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };

    super::detect_format(path, &bytes).is_ok()
}

fn source_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("eml") => "email",
        _ => "document",
    }
}

fn should_ingest_path(
    path: &Path,
    last_seen: &mut HashMap<PathBuf, Instant>,
    now: Instant,
    interval: Duration,
) -> bool {
    let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    if let Some(previous) = last_seen.get(&key)
        && now.duration_since(*previous) < interval
    {
        return false;
    }

    last_seen.insert(key, now);
    true
}

#[cfg(test)]
mod tests {
    use notify::{
        Event,
        event::{CreateKind, EventKind, ModifyKind, RemoveKind},
    };
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn watched_paths_for_event_keeps_supported_create_and_modify_files() {
        let temp_dir = tempdir().expect("temp dir should exist");
        let markdown_path = temp_dir.path().join("note.md");
        std::fs::write(&markdown_path, "watch me").expect("fixture should write");

        let create_paths = watched_paths_for_event(&Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![markdown_path.clone()],
            attrs: Default::default(),
        });
        let modify_paths = watched_paths_for_event(&Event {
            kind: EventKind::Modify(ModifyKind::Data(notify::event::DataChange::Content)),
            paths: vec![markdown_path.clone()],
            attrs: Default::default(),
        });

        assert_eq!(create_paths, vec![markdown_path.clone()]);
        assert_eq!(modify_paths, vec![markdown_path]);
    }

    #[test]
    fn watched_paths_for_event_ignores_remove_events_and_unsupported_files() {
        let temp_dir = tempdir().expect("temp dir should exist");
        let unsupported_path = temp_dir.path().join("ignored.json");
        std::fs::write(&unsupported_path, "{}").expect("unsupported fixture should write");

        let remove_paths = watched_paths_for_event(&Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![unsupported_path.clone()],
            attrs: Default::default(),
        });
        let unsupported_paths = watched_paths_for_event(&Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![unsupported_path],
            attrs: Default::default(),
        });

        assert!(remove_paths.is_empty());
        assert!(unsupported_paths.is_empty());
    }

    #[test]
    fn source_type_for_path_uses_email_for_eml_files() {
        assert_eq!(
            source_type_for_path(&PathBuf::from("/tmp/mail.eml")),
            "email"
        );
        assert_eq!(
            source_type_for_path(&PathBuf::from("/tmp/note.md")),
            "document"
        );
    }

    #[test]
    fn should_ingest_path_applies_interval_per_file() {
        let path = PathBuf::from("/tmp/note.md");
        let mut last_seen = HashMap::new();
        let now = Instant::now();

        assert!(should_ingest_path(
            &path,
            &mut last_seen,
            now,
            Duration::from_secs(5)
        ));
        assert!(!should_ingest_path(
            &path,
            &mut last_seen,
            now + Duration::from_secs(2),
            Duration::from_secs(5)
        ));
        assert!(should_ingest_path(
            &path,
            &mut last_seen,
            now + Duration::from_secs(5),
            Duration::from_secs(5)
        ));
    }
}
