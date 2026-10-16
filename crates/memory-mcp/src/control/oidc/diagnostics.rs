//! Bounded, per-provider state transitions and completed upstream attempts.
use std::{sync::Mutex, time::Duration};

macro_rules! vocabulary {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
        #[serde(rename_all="snake_case")]
        #[allow(dead_code, reason="complete bounded diagnostic vocabulary, including disabled providers which emit no events")]
        pub enum $name { $($variant),+ }
        impl $name { pub(crate) fn label(self) -> &'static str { match self { $(Self::$variant => $label),+ } } }
    }
}
vocabulary!(ProviderComponent { Metadata=>"metadata", Jwks=>"jwks", Token=>"token" });
vocabulary!(ProviderState { Disabled=>"disabled", Initializing=>"initializing", Ready=>"ready", Backoff=>"backoff", InvalidResponse=>"invalid_response", Empty=>"empty", Fresh=>"fresh", Stale=>"stale" });
vocabulary!(ProviderStage { Discovery=>"discovery", Jwks=>"jwks", Token=>"token" });
vocabulary!(ProviderOutcome { Success=>"success", Unavailable=>"unavailable", InvalidResponse=>"invalid_response", Refused=>"refused" });
#[derive(Default)]
pub(crate) struct ProviderDiagnostics {
    states: Mutex<[Option<ProviderState>; 3]>,
}
impl ProviderDiagnostics {
    pub(crate) fn transition(
        &self,
        component: ProviderComponent,
        state: ProviderState,
        reason: &'static str,
        status: Option<u16>,
        retry_after: Duration,
    ) {
        let Ok(mut states) = self.states.lock() else {
            return;
        };
        let slot = &mut states[component as usize];
        let changed = *slot != Some(state);
        *slot = Some(state);
        drop(states);
        let degraded = matches!(
            state,
            ProviderState::Backoff | ProviderState::InvalidResponse | ProviderState::Stale
        );
        let level = if !changed {
            crate::logging::LogLevel::Debug
        } else if degraded {
            crate::logging::LogLevel::Warn
        } else {
            crate::logging::LogLevel::Info
        };
        crate::logging::emit(
            std::collections::HashMap::from([
                (
                    "op".into(),
                    serde_json::json!("oidc.provider.state_changed"),
                ),
                ("component".into(), serde_json::json!(component.label())),
                ("state".into(), serde_json::json!(state.label())),
                ("reason".into(), serde_json::json!(reason)),
                ("status".into(), serde_json::json!(status)),
                (
                    "retry_after_secs".into(),
                    serde_json::json!(retry_after.as_secs()),
                ),
            ]),
            level,
        );
    }
    pub(crate) fn ready(&self, component: ProviderComponent, ready: bool) {
        if component != ProviderComponent::Token {
            crate::observability::record_oidc_ready(component.label(), ready);
        }
    }
    pub(crate) fn request_finished(&self, stage: ProviderStage, outcome: ProviderOutcome) {
        crate::observability::record_oidc_request(stage.label(), outcome.label());
    }
}
pub(crate) struct RequestObservation<'a> {
    diagnostics: &'a ProviderDiagnostics,
    stage: ProviderStage,
    outcome: ProviderOutcome,
}
impl<'a> RequestObservation<'a> {
    pub(crate) fn new(diagnostics: &'a ProviderDiagnostics, stage: ProviderStage) -> Self {
        Self {
            diagnostics,
            stage,
            outcome: ProviderOutcome::Unavailable,
        }
    }
    pub(crate) fn finish(&mut self, outcome: ProviderOutcome) {
        self.outcome = outcome;
    }
}
impl Drop for RequestObservation<'_> {
    fn drop(&mut self) {
        self.diagnostics.request_finished(self.stage, self.outcome);
    }
}
impl super::provider::ProviderFailure {
    pub(crate) fn outcome(&self) -> ProviderOutcome {
        if self.kind == super::provider::ProviderFailureKind::InvalidResponse {
            ProviderOutcome::InvalidResponse
        } else if self.status.is_some_and(|s| s < 500 && s != 429) {
            ProviderOutcome::Refused
        } else {
            ProviderOutcome::Unavailable
        }
    }
    pub(crate) fn reason(&self) -> &'static str {
        match self.kind {
            super::provider::ProviderFailureKind::Transport => "transport",
            super::provider::ProviderFailureKind::Timeout => "timeout",
            super::provider::ProviderFailureKind::Http => "http",
            super::provider::ProviderFailureKind::InvalidResponse => "invalid_response",
        }
    }
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataStatus {
    Disabled,
    Initializing,
    Ready,
    Backoff,
    InvalidResponse,
}
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct OidcHealth {
    pub enabled: bool,
    pub metadata: MetadataStatus,
    pub jwks: super::JwksStatus,
}
impl OidcHealth {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            metadata: MetadataStatus::Disabled,
            jwks: super::JwksStatus::Disabled,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn repeated_degradation_logs_once_then_recovery() {
        let _serialized = crate::logging::capture::SERIAL.lock().await;
        let _level = crate::logging::capture::LevelOverride::install("info");
        let sink = crate::logging::capture::install();
        let d = ProviderDiagnostics::default();
        d.transition(
            ProviderComponent::Metadata,
            ProviderState::Backoff,
            "http",
            Some(502),
            Duration::from_secs(1),
        );
        d.transition(
            ProviderComponent::Metadata,
            ProviderState::Backoff,
            "http",
            Some(502),
            Duration::from_secs(2),
        );
        d.transition(
            ProviderComponent::Metadata,
            ProviderState::Ready,
            "success",
            None,
            Duration::ZERO,
        );
        let lines = sink.lines();
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("op=oidc.provider.state_changed") && l.contains("WARN"))
                .count(),
            1
        );
        assert_eq!(
            lines
                .iter()
                .filter(|l| l.contains("op=oidc.provider.state_changed") && l.contains("INFO"))
                .count(),
            1
        );
    }
    #[cfg(feature = "prometheus")]
    #[tokio::test]
    async fn completed_requests_are_counted_without_identity_labels() {
        use crate::observability::tests::sample_series;
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let d = ProviderDiagnostics::default();
            let labels = "stage=\"discovery\",outcome=\"unavailable\"";
            let before = sample_series(
                &handle.render(),
                "memory_oidc_provider_requests_total",
                labels,
            )
            .unwrap_or(0.);
            d.request_finished(ProviderStage::Discovery, ProviderOutcome::Unavailable);
            let after = handle.render();
            assert_eq!(
                sample_series(&after, "memory_oidc_provider_requests_total", labels),
                Some(before + 1.)
            );
            assert!(
                !after
                    .lines()
                    .filter(|l| l.starts_with("memory_oidc_provider_"))
                    .any(|l| l.contains("kid=")
                        || l.contains("issuer=")
                        || l.contains("fixture-secret-token"))
            );
        });
    }
}
