//! The four mutations the client detail page offers.
//!
//! They are methods on [`PageState`] rather than free functions because they are
//! one object's worth of behaviour: all four share one in-flight slot, one
//! compare-and-set version and one re-authentication refusal, and every one of
//! them reports through the same two signals. A page-level helper that took each
//! of those as a parameter would be the same struct with more room to disagree.
//!
//! Two rules are worth stating because they are easy to get wrong:
//!
//! * issuance is never retried automatically. The operator presses the button
//!   again, and *that* is what reuses the operation id — so a lost response is
//!   answered by the backend with `secret_already_issued` instead of a second
//!   secret;
//! * re-authentication rotates the session, so the new CSRF token is read before
//!   anything else is attempted, including a later issuance.

use dioxus::prelude::*;

use crate::admin_api::{
    AdminApi, AdminApiError, ClientStateAction, SessionCsrf, fresh_operation_id,
    keeps_operation_id, sanitize_note,
};
use crate::state::admin_session::{AdminSession, SESSION_ENDED, SESSION_ENDED_BEFORE_MUTATION};

use super::state::PageState;

/// A mutation the backend refused with `reauth_required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RefusedMutation {
    Suspend,
    Resume,
    /// Key issuance is never retried automatically: the operator confirms the
    /// password, then presses the button again with the same operation id.
    IssueKey,
}

impl RefusedMutation {
    pub(super) fn state_action(self) -> Option<ClientStateAction> {
        match self {
            Self::Suspend => Some(ClientStateAction::Suspend),
            Self::Resume => Some(ClientStateAction::Resume),
            Self::IssueKey => None,
        }
    }

    pub(super) fn reason(self) -> &'static str {
        match self {
            Self::Suspend => "suspend this client",
            Self::Resume => "resume this client",
            Self::IssueKey => "issue this key",
        }
    }
}

/// The confirmation shown after a successful state change.
pub(super) fn state_change_notice(action: ClientStateAction) -> &'static str {
    match action {
        ClientStateAction::Suspend => "Client suspended.",
        ClientStateAction::Resume => "Client resumed.",
    }
}

/// Record a failure: session-wide problems end the page's session, everything
/// else is shown in place.
pub(super) fn record_failure(
    mut session: Signal<AdminSession>,
    mut error: Signal<Option<String>>,
    failure: AdminApiError,
) {
    if failure.ends_session() {
        session.write().mark_ended(SESSION_ENDED.to_owned());
    } else {
        error.set(Some(failure.user_message().to_owned()));
    }
}

impl PageState {
    /// Re-read the client and the keys, superseding any poll in flight.
    pub(super) fn reload(&mut self) {
        crate::state::query::bump_generation(&mut self.client_generation);
        let mut resource = self.client_resource;
        resource.restart();
        self.keys.first_page();
    }

    /// End the session in place, for a mutation that cannot even be attempted.
    fn refuse_for_missing_session(&mut self) {
        self.pending.set(false);
        self.error
            .set(Some(SESSION_ENDED_BEFORE_MUTATION.to_owned()));
        self.session.write().mark_ended(SESSION_ENDED.to_owned());
    }

    /// Issue one key for this client.
    ///
    /// One deliberate action: the operation id is generated once and reused by
    /// every manual retry of this same request.
    pub(super) fn issue_key(&mut self, account_id: &str) {
        if *self.pending.peek() {
            return;
        }
        let name = match self.fields.validated_name() {
            Ok(name) => name,
            Err(message) => {
                self.error.set(Some(message.to_owned()));
                return;
            }
        };
        let expiry = match self.fields.validated_expiry() {
            Ok(expiry) => expiry,
            Err(message) => {
                self.error.set(Some(message.to_owned()));
                return;
            }
        };
        let existing = self.operation.peek().clone();
        let operation_id = match existing {
            Some(existing) => existing,
            None => match fresh_operation_id() {
                Ok(fresh) => {
                    self.operation.set(Some(fresh.clone()));
                    fresh
                }
                Err(failure) => {
                    self.error.set(Some(failure.user_message().to_owned()));
                    return;
                }
            },
        };
        let Some(api) = self.session.peek().mutation_client() else {
            self.refuse_for_missing_session();
            return;
        };
        self.pending.set(true);
        self.error.set(None);
        self.notice.set(None);
        let id = account_id.to_owned();
        let mut state = *self;
        spawn(async move {
            match api.issue_key(&id, &name, &expiry, &operation_id).await {
                Ok(created) => {
                    // The idempotency record is finished with, and the form is
                    // cleared only now: a retry of a lost response must still be
                    // able to resend what was typed.
                    state.operation.set(None);
                    state.fields.clear();
                    state.notice.set(None);
                    state.secret.set(Some(created));
                    state.keys.first_page();
                }
                Err(failure) if failure.is_reauth_required() => {
                    // Confirm the password first; issuance is retried by the
                    // operator, never by this code.
                    state.refused.set(Some(RefusedMutation::IssueKey));
                }
                Err(failure) if failure.code == "secret_already_issued" => {
                    state.operation.set(None);
                    let key_hint = failure
                        .key_id
                        .as_deref()
                        .map(sanitize_note)
                        .filter(|id| !id.is_empty());
                    state.error.set(Some(match key_hint {
                        Some(key_id) => format!(
                            "A secret was already issued for key {key_id}. Revoke that key, then issue a new one."
                        ),
                        None => failure.user_message().to_owned(),
                    }));
                    state.keys.first_page();
                }
                Err(failure) => {
                    if !keeps_operation_id(&failure) {
                        state.operation.set(None);
                    }
                    if failure.code == "conflict" {
                        // The client moved under us: reload before the operator
                        // tries again.
                        state.reload();
                    }
                    state.error.set(Some(failure.user_message().to_owned()));
                }
            }
            state.pending.set(false);
        });
    }

    /// Revoke one key.
    pub(super) fn revoke_key(&mut self, account_id: &str, key_id: String) {
        if *self.pending.peek() {
            return;
        }
        let Some(api) = self.session.peek().mutation_client() else {
            self.refuse_for_missing_session();
            return;
        };
        self.pending.set(true);
        self.error.set(None);
        let id = account_id.to_owned();
        let mut state = *self;
        spawn(async move {
            match api.revoke_key(&id, &key_id).await {
                Ok(()) => {
                    state.notice.set(Some("Key revoked.".to_owned()));
                    state.keys.first_page();
                }
                Err(failure) => {
                    if failure.ends_session() {
                        state.session.write().mark_ended(SESSION_ENDED.to_owned());
                    } else {
                        state.error.set(Some(failure.user_message().to_owned()));
                        state.keys.first_page();
                    }
                }
            }
            state.pending.set(false);
        });
    }

    /// Suspend or resume the client, against the version the page last read.
    pub(super) fn run_state_change(&mut self, account_id: &str, action: ClientStateAction) {
        if *self.pending.peek() {
            return;
        }
        let Some(version) = self.client.peek().as_ref().map(|view| view.version) else {
            self.error.set(Some(
                "Reload the client before changing its state.".to_owned(),
            ));
            return;
        };
        let Some(api) = self.session.peek().mutation_client() else {
            self.refuse_for_missing_session();
            return;
        };
        self.pending.set(true);
        self.error.set(None);
        self.notice.set(None);
        self.confirm_action.set(None);
        let id = account_id.to_owned();
        let mut state = *self;
        spawn(async move {
            match api.set_state(&id, version, action).await {
                Ok(()) => {
                    state
                        .notice
                        .set(Some(state_change_notice(action).to_owned()));
                    state.reload();
                }
                Err(failure) if failure.is_reauth_required() => {
                    state.refused.set(Some(match action {
                        ClientStateAction::Suspend => RefusedMutation::Suspend,
                        ClientStateAction::Resume => RefusedMutation::Resume,
                    }));
                }
                Err(failure) => {
                    if failure.code == "conflict" {
                        // Stale `expected_version`: show the current state
                        // instead of retrying with a guessed version.
                        state.reload();
                    }
                    state.error.set(Some(failure.user_message().to_owned()));
                }
            }
            state.pending.set(false);
        });
    }

    /// Continue the mutation the backend refused, now that the password has been
    /// re-entered.
    ///
    /// Issuance is the exception: nothing is retried for it, because a retry is
    /// the operator's deliberate action reusing the operation id.
    pub(super) fn confirm_reauth(&mut self, account_id: &str) {
        let Some(action) = *self.refused.peek() else {
            return;
        };
        self.refused.set(None);
        self.pending.set(true);
        let id = account_id.to_owned();
        let mut state = *self;
        spawn(async move {
            // Reauthentication rotated the session, so the new CSRF token has to
            // be read before anything else is attempted.
            let response = match AdminApi::new().session().await {
                Ok(response) => response,
                Err(failure) => {
                    state.pending.set(false);
                    state.error.set(Some(failure.user_message().to_owned()));
                    return;
                }
            };
            let api =
                AdminApi::new().with_session_csrf(SessionCsrf::new(response.csrf_token.clone()));
            state.session.write().adopt(response);
            let Some(state_action) = action.state_action() else {
                state.pending.set(false);
                state.notice.set(Some(
                    "Password confirmed. Press “Issue key” again to send that same request."
                        .to_owned(),
                ));
                return;
            };
            let version = state
                .client
                .peek()
                .as_ref()
                .map(|view| view.version)
                .unwrap_or(0);
            match api.set_state(&id, version, state_action).await {
                Ok(()) => {
                    state
                        .notice
                        .set(Some(state_change_notice(state_action).to_owned()));
                    state.reload();
                }
                Err(failure) => state.error.set(Some(failure.user_message().to_owned())),
            }
            state.pending.set(false);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{RefusedMutation, state_change_notice};
    use crate::admin_api::ClientStateAction;

    #[test]
    fn refused_mutations_map_to_their_state_action() {
        assert_eq!(
            RefusedMutation::Suspend.state_action(),
            Some(ClientStateAction::Suspend)
        );
        assert_eq!(
            RefusedMutation::Resume.state_action(),
            Some(ClientStateAction::Resume)
        );
        assert_eq!(RefusedMutation::IssueKey.state_action(), None);
    }

    #[test]
    fn every_refusal_names_the_operation_it_refused() {
        for refusal in [
            RefusedMutation::Suspend,
            RefusedMutation::Resume,
            RefusedMutation::IssueKey,
        ] {
            assert!(
                refusal
                    .reason()
                    .starts_with(|first: char| first.is_lowercase())
            );
            assert!(!refusal.reason().ends_with('.'));
        }
    }

    #[test]
    fn a_state_change_reports_which_change_happened() {
        assert_eq!(
            state_change_notice(ClientStateAction::Suspend),
            "Client suspended."
        );
        assert_eq!(
            state_change_notice(ClientStateAction::Resume),
            "Client resumed."
        );
    }
}
