//! Memory extraction use case: rate-limit first, verify the
//! episode exists, then delegate the extraction itself.

use std::sync::{Arc, Mutex};

use memory_mcp::MemoryError;
use memory_mcp::memory::api::{
    EpisodeExtractionPort, ExtractCommand, ExtractedEpisode, RateLimitPort, extract_from_episode,
};
use memory_mcp::models::ExtractResult;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ExtractionCall {
    episode_id: String,
    zero_shot_labels: Option<Vec<String>>,
}

struct RecordingPort {
    present: Mutex<Vec<String>>,
    calls: Mutex<Vec<ExtractionCall>>,
    result: ExtractResult,
}

impl RecordingPort {
    fn knowing(episode_ids: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            present: Mutex::new(episode_ids.iter().map(|id| (*id).to_owned()).collect()),
            calls: Mutex::new(Vec::new()),
            result: ExtractResult::default(),
        })
    }

    fn calls(&self) -> Vec<ExtractionCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

#[async_trait::async_trait]
impl EpisodeExtractionPort for RecordingPort {
    async fn episode_exists(&self, episode_id: &str) -> Result<bool, MemoryError> {
        Ok(self
            .present
            .lock()
            .expect("present lock")
            .iter()
            .any(|id| id == episode_id))
    }

    async fn extract(&self, command: &ExtractCommand<'_>) -> Result<ExtractedEpisode, MemoryError> {
        self.calls.lock().expect("calls lock").push(ExtractionCall {
            episode_id: command.episode_id.clone(),
            zero_shot_labels: command.zero_shot_labels.map(<[String]>::to_vec),
        });
        Ok(ExtractedEpisode {
            episode: None,
            result: self.result.clone(),
        })
    }
}

struct AllowAll;

impl RateLimitPort for AllowAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Ok(())
    }
}

struct DenyAll;

impl RateLimitPort for DenyAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Err(MemoryError::Validation("rate limit exceeded".into()))
    }
}

fn command(episode_id: &str) -> ExtractCommand<'_> {
    ExtractCommand {
        episode_id: episode_id.to_owned(),
        zero_shot_labels: None,
        caller_id: Some("user-1".to_owned()),
    }
}

#[tokio::test]
async fn extraction_delegates_the_episode_and_optional_labels() {
    let port = RecordingPort::knowing(&["episode:abc"]);

    let result = extract_from_episode(port.as_ref(), &AllowAll, &command("episode:abc"))
        .await
        .expect("extraction succeeds");

    assert_eq!(result.result, ExtractResult::default());
    assert!(
        result.episode.is_none(),
        "no episode is seeded for this fake"
    );
    assert_eq!(
        port.calls(),
        vec![ExtractionCall {
            episode_id: "episode:abc".to_owned(),
            zero_shot_labels: None,
        }],
        "the episode ID and labels travel verbatim"
    );
}

#[tokio::test]
async fn custom_zero_shot_labels_are_forwarded() {
    let port = RecordingPort::knowing(&["episode:abc"]);
    let labels = vec!["person".to_owned(), "project".to_owned()];

    let mut cmd = command("episode:abc");
    cmd.zero_shot_labels = Some(&labels);

    extract_from_episode(port.as_ref(), &AllowAll, &cmd)
        .await
        .expect("extraction succeeds");

    assert_eq!(
        port.calls()[0].zero_shot_labels,
        Some(labels),
        "caller-supplied NER labels are not dropped"
    );
}

#[tokio::test]
async fn a_missing_episode_is_reported_with_its_id_and_never_extracted() {
    let port = RecordingPort::knowing(&[]);

    let error = extract_from_episode(port.as_ref(), &AllowAll, &command("episode:nope"))
        .await
        .expect_err("a missing episode cannot be extracted");

    assert!(
        matches!(&error, MemoryError::NotFound(message) if message.contains("episode:nope")),
        "the not-found message must name the episode, got {error:?}"
    );
    assert!(
        port.calls().is_empty(),
        "extraction must not run for a missing episode"
    );
}

#[tokio::test]
async fn a_refused_caller_never_looks_up_or_extracts() {
    let port = RecordingPort::knowing(&["episode:abc"]);

    let error = extract_from_episode(port.as_ref(), &DenyAll, &command("episode:abc"))
        .await
        .expect_err("a rate-limited caller cannot extract");

    assert!(matches!(&error, MemoryError::Validation(_)));
    assert!(
        port.calls().is_empty(),
        "a rate-limited caller must not run extraction"
    );
}

#[tokio::test]
async fn an_extraction_with_nothing_to_report_is_still_a_success() {
    let port = RecordingPort::knowing(&["episode:abc"]);

    let result = extract_from_episode(port.as_ref(), &AllowAll, &command("episode:abc"))
        .await
        .expect("an empty extraction is not an error");

    assert_eq!(result.result, ExtractResult::default());
    assert!(result.result.entities.is_empty() && result.result.facts.is_empty());
}

#[tokio::test]
async fn a_present_episode_runs_extraction_exactly_once() {
    let port = RecordingPort::knowing(&["episode:abc"]);

    extract_from_episode(port.as_ref(), &AllowAll, &command("episode:abc"))
        .await
        .expect("extraction succeeds");

    assert_eq!(
        port.calls().len(),
        1,
        "the model-backed extractor runs once per request"
    );
}
