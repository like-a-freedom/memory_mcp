//! The client detail page's state, and the hooks that own it.
//!
//! One page, four reasons to change: this module loads the client and keeps it
//! current, `mutations.rs` performs the four mutations the page offers,
//! `keys_panel.rs` renders one client's keys, and `mod.rs` composes them. The
//! state lives here because all four read it.
//!
//! [`PageState`] is one struct rather than a dozen locals for a plain reason: the
//! mutation flows are methods on it, and a method cannot borrow a local its
//! caller still holds. It is `Copy`, like the signal handles inside it, so a
//! handler closure can take a copy instead of borrowing the page.

use dioxus::prelude::*;

use crate::admin_api::{
    AdminApi, AdminApiError, ApiKeyMeta, ClientStateAction, ClientView, CreatedKey, KeyExpiry,
    OperationId, parse_expiry, sleep_ms, validate_name,
};
use crate::state::admin_session::{AdminSession, use_console_session};
use crate::state::paged_query::{PagedQuery, use_paged_query};
use crate::state::polling::{POLL_INTERVAL_MS, next_backoff};

use super::mutations::{RefusedMutation, record_failure};

/// Keys requested per page; the backend documents 50 with a maximum of 100.
const KEY_PAGE_LIMIT: u16 = 50;

/// The issue-key fields, owned by the page even though the panel renders them.
///
/// A manual retry of a lost issuance resends the same name and expiry with the
/// same operation id, so the values have to outlive the form's own submit; an
/// *accepted* issuance is what clears them.
///
/// Three signal handles in one struct rather than three locals because the panel
/// that renders them is a component: the page keeps ownership, and the panel
/// reads and writes the same handles.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct KeyIssueFields {
    pub(super) name: Signal<String>,
    pub(super) days: Signal<String>,
    pub(super) never: Signal<bool>,
    /// A rejection of what the form holds, rendered inside the form. The
    /// page-level error is for loads and mutations; telling an operator their
    /// input is wrong 600px above the input is telling them nothing.
    pub(super) error: Signal<Option<String>>,
}

impl KeyIssueFields {
    fn new() -> Self {
        Self {
            name: use_signal(String::new),
            days: use_signal(String::new),
            never: use_signal(|| false),
            error: use_signal(|| None),
        }
    }

    /// A typed day count and "Never" are mutually exclusive choices, not a pair
    /// to reconcile: typing a count takes the box off the table.
    pub(super) fn enter_days(&mut self, value: String) {
        let (days, never) = entered_days(value, *self.never.peek());
        self.days.set(days);
        self.never.set(never);
    }

    /// Checking "Never" clears the day count instead of contradicting it.
    pub(super) fn toggle_never(&mut self, checked: bool) {
        let (days, never) = toggled_never(checked, self.days.read().clone());
        self.days.set(days);
        self.never.set(never);
    }

    /// The name to send, validated by the rule the backend also applies.
    ///
    /// The error is the sentence to show: it is fixed copy from `admin_api`, not
    /// text the backend sent.
    pub(super) fn validated_name(&self) -> Result<String, &'static str> {
        validate_name(&self.name.read()).map_err(|rejection| rejection.message())
    }

    /// The expiry to send. There is no silent default: a blank choice and a
    /// contradictory one are both rejected here, before a request is built.
    pub(super) fn validated_expiry(&self) -> Result<KeyExpiry, &'static str> {
        parse_expiry(*self.never.peek(), &self.days.read()).map_err(|rejection| rejection.message())
    }

    /// Forget what was typed, after an issuance was accepted.
    pub(super) fn clear(&mut self) {
        self.name.set(String::new());
        self.days.set(String::new());
        self.never.set(false);
        self.error.set(None);
    }
}

/// The expiry interaction, as data: a day count implies "not never".
///
/// Empty days change nothing — there is no default, and the blank pair is
/// still the operator's decision to make.
pub(super) fn entered_days(days: String, never: bool) -> (String, bool) {
    let never = if days.trim().is_empty() { never } else { false };
    (days, never)
}

/// The other direction: "Never" empties the day count rather than
/// contradicting it.
pub(super) fn toggled_never(checked: bool, days: String) -> (String, bool) {
    (if checked { String::new() } else { days }, checked)
}

/// Everything the client detail page renders or mutates.
#[derive(Clone, Copy)]
pub(super) struct PageState {
    pub(super) session: Signal<AdminSession>,
    /// The client as last read. `None` until the first read answers.
    pub(super) client: Signal<Option<ClientView>>,
    /// Names one deliberate client read, so a poll that a manual refresh
    /// overtook is recognised and dropped.
    pub(super) client_generation: Signal<u64>,
    pub(super) client_resource: Resource<Result<ClientView, AdminApiError>>,
    /// One page of this client's keys, with the pager that walks it.
    pub(super) keys: PagedQuery<ApiKeyMeta>,
    pub(super) fields: KeyIssueFields,
    /// The idempotency id of the issuance in progress, kept across a manual
    /// retry so a lost response cannot mint a second secret.
    pub(super) operation: Signal<Option<OperationId>>,
    pub(super) secret: Signal<Option<CreatedKey>>,
    pub(super) error: Signal<Option<String>>,
    pub(super) notice: Signal<Option<String>>,
    /// The one in-flight slot every mutation on this page shares.
    pub(super) pending: Signal<bool>,
    /// The state change awaiting confirmation.
    pub(super) confirm_action: Signal<Option<ClientStateAction>>,
    /// The mutation the backend refused with `reauth_required`.
    pub(super) refused: Signal<Option<RefusedMutation>>,
}

/// Read the client, keep it current while it provisions, and page its keys.
///
/// Requires the console layout's session. The poll's task belongs to this scope,
/// so unmounting the page cancels it: nothing here polls a page nobody is
/// looking at.
pub(super) fn use_page_state(account_id: String) -> PageState {
    let session = use_console_session();
    let mut client = use_signal(|| None::<ClientView>);
    let client_generation = use_signal(|| 0_u64);
    let error = use_signal(|| None::<String>);
    let notice = use_signal(|| None::<String>);
    let secret = use_signal(|| None::<CreatedKey>);
    let operation = use_signal(|| None::<OperationId>);
    let pending = use_signal(|| false);
    let confirm_action = use_signal(|| None::<ClientStateAction>);
    let refused = use_signal(|| None::<RefusedMutation>);
    let fields = KeyIssueFields::new();

    let keys = use_paged_query({
        let account_id = account_id.clone();
        move |cursor| {
            let id = account_id.clone();
            async move {
                AdminApi::new()
                    .keys(&id, cursor.as_deref(), KEY_PAGE_LIMIT)
                    .await
            }
        }
    });

    let client_resource = use_resource({
        let account_id = account_id.clone();
        move || {
            let id = account_id.clone();
            async move { AdminApi::new().client(&id).await }
        }
    });

    use_effect(move || {
        let Some(outcome) = client_resource.read().clone() else {
            return;
        };
        match outcome {
            Ok(view) => client.set(Some(view)),
            Err(failure) => record_failure(session, error, failure),
        }
    });

    // Keep the readiness display current while the client is provisioning, and
    // back off when the backend cannot answer.
    use_future({
        let account_id = account_id.clone();
        move || {
            let id = account_id.clone();
            let mut client = client;
            let generation = client_generation;
            async move {
                let mut backoff = 0_u32;
                loop {
                    let delay = if backoff == 0 {
                        POLL_INTERVAL_MS
                    } else {
                        backoff
                    };
                    sleep_ms(delay).await;
                    if !client
                        .peek()
                        .as_ref()
                        .is_some_and(ClientView::is_provisioning)
                    {
                        continue;
                    }
                    let asked = *generation.peek();
                    match AdminApi::new().client(&id).await {
                        Ok(view) if *generation.peek() == asked => {
                            backoff = 0;
                            client.set(Some(view));
                        }
                        Ok(_) => {
                            // A manual refresh won while this poll was in flight;
                            // its answer is no longer the authoritative one.
                            backoff = 0;
                        }
                        Err(_) => backoff = next_backoff(backoff),
                    }
                }
            }
        }
    });

    PageState {
        session,
        client,
        client_generation,
        client_resource,
        keys,
        fields,
        operation,
        secret,
        error,
        notice,
        pending,
        confirm_action,
        refused,
    }
}

#[cfg(test)]
mod tests {
    use super::{entered_days, toggled_never};

    #[test]
    fn typing_a_day_count_takes_never_off_the_table() {
        let (days, never) = entered_days("30".to_owned(), true);
        assert_eq!(days, "30");
        assert!(!never);
    }

    #[test]
    fn clearing_the_day_count_does_not_check_never_by_itself() {
        // There is no default: a blank pair stays blank, and the submit-side
        // validation is what says so.
        let (_, never) = entered_days(String::new(), false);
        assert!(!never);
    }

    #[test]
    fn checking_never_clears_the_day_count_instead_of_contradicting_it() {
        let (days, never) = toggled_never(true, "30".to_owned());
        assert_eq!(days, "");
        assert!(never);
        let (days, never) = toggled_never(false, "30".to_owned());
        assert_eq!(days, "30");
        assert!(!never);
    }
}
