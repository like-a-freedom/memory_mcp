//! One paged read, shared by the two signed-in console pages.
//!
//! The client list and one client's keys are the same read twice: a
//! cursor-paginated collection the backend answers asynchronously, which a poll
//! may re-read, and whose answer must be recognised as stale when a newer
//! request overtook it. Three rules apply to both, and each rule is subtle
//! enough that two copies would eventually disagree:
//!
//! * a request is named by a generation, and only the answer carrying the
//!   generation the pager currently holds is applied;
//! * a failure that proves the session is gone ends the session instead of being
//!   painted as a page error;
//! * a poll re-reads the page already on screen, so it never moves the pager.
//!
//! This module is the only place those rules are written.

use std::future::Future;

use dioxus::prelude::*;

use crate::admin_api::{AdminApiError, Page};
use crate::state::admin_session::{SESSION_ENDED, use_console_session};
use crate::state::query::{PageDirection, PageRequest, Paged, request_page};

/// A cursor-paginated read, the pager that walks it, and the request in flight.
///
/// `Copy` like the signals it holds, so a handler closure can own one without
/// borrowing the page it belongs to.
pub struct PagedQuery<T: 'static> {
    /// Rows, pager position and the last failure. The page renders this.
    pub page: Signal<Paged<T>>,
    request: Signal<PageRequest>,
}

impl<T: 'static> Clone for PagedQuery<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: 'static> Copy for PagedQuery<T> {}

impl<T: 'static> PagedQuery<T> {
    /// The generation naming the request in flight.
    pub fn generation(&self) -> u64 {
        self.request.peek().generation()
    }

    /// Whether `generation` still names the request in flight.
    ///
    /// A poll compares the generation it was issued under with this before it
    /// writes anything: an answer that lost the race describes rows the operator
    /// is no longer looking at.
    pub fn is_current(&self, generation: u64) -> bool {
        self.generation() == generation
    }

    /// Ask for one page, superseding whatever request is in flight.
    pub fn goto(&mut self, cursor: Option<String>, direction: PageDirection) {
        request_page(&mut self.request, self.page, cursor, direction);
    }

    /// Ask for the first page again.
    pub fn first_page(&mut self) {
        self.goto(None, PageDirection::Replace);
    }

    /// Ask for the page after the one on screen, when the backend reported one.
    pub fn next(&mut self) {
        let cursor = self.page.peek().next_cursor();
        self.goto(cursor, PageDirection::Next);
    }

    /// Ask for the page before the one on screen, when there is one.
    pub fn previous(&mut self) {
        let cursor = self.page.peek().previous_cursor();
        if let Some(cursor) = cursor {
            self.goto(cursor, PageDirection::Previous);
        }
    }

    /// The cursor of the page on screen; `None` is the first page.
    pub fn cursor(&self) -> Option<String> {
        self.page.peek().cursor().map(ToOwned::to_owned)
    }

    /// Replace the rows on screen with the ones a poll just read.
    ///
    /// A poll reads the page the operator is looking at, so the request it is
    /// recorded under keeps the current cursor and never advances the pager.
    pub fn accept_poll(&mut self, rows: Page<T>, generation: u64) {
        let cursor = self.cursor();
        self.page
            .write()
            .accept(rows, PageRequest::refresh(cursor, generation));
    }

    /// Report a poll's failure, unless a newer request has superseded it.
    ///
    /// The guard is inside on purpose: a poll that lost a race must not report
    /// its own failure over the newer page, and a caller cannot forget to check.
    pub fn fail_poll(&mut self, generation: u64, message: String) {
        if self.is_current(generation) {
            self.page.write().fail(message);
        }
    }
}

/// Read one page per [`PageRequest`], applying the answer that is still current.
///
/// `fetch` is called with the cursor to read (`None` for the first page), and is
/// recreated on every render like any other resource closure: it must be cheap
/// and must not read the pager itself.
///
/// Requires the console layout's session, which is where a read the backend
/// refuses for a dead session is ended rather than shown as a page error.
pub fn use_paged_query<T, F, Fut>(mut fetch: F) -> PagedQuery<T>
where
    // `Clone` because an answer is read out of the resource's slot before it can
    // be applied: the slot cannot hand over ownership of what it holds.
    T: Clone + 'static,
    F: FnMut(Option<String>) -> Fut + 'static,
    Fut: Future<Output = Result<Page<T>, AdminApiError>> + 'static,
{
    let mut page = use_signal(Paged::<T>::default);
    let request = use_signal(PageRequest::default);
    let mut session = use_console_session();

    // The handle is dropped at the end of this call: the resource's task belongs
    // to this component's scope and re-runs whenever `request` changes, so
    // nothing here needs to reach for `restart`.
    let resource = use_resource(move || {
        let asked = request.read().clone();
        let cursor = asked.cursor().map(ToOwned::to_owned);
        let answer = fetch(cursor);
        async move {
            match answer.await {
                Ok(rows) => Ok((rows, asked)),
                Err(failure) => Err((asked, failure)),
            }
        }
    });

    use_effect(move || {
        let Some(answer) = resource.read().clone() else {
            // Not answered yet. `begin` keeps the rows already on screen while
            // marking the fetch as running, so the pager and the display cannot
            // disagree about whether a request is in flight.
            page.write().begin();
            return;
        };
        let current = request.peek().generation();
        match answer {
            Ok((rows, asked)) if asked.generation() == current => page.write().accept(rows, asked),
            Ok(_) => {}
            Err((asked, failure)) if asked.generation() == current => {
                if failure.ends_session() {
                    session.write().mark_ended(SESSION_ENDED.to_owned());
                } else {
                    page.write().fail(failure.user_message().to_owned());
                }
            }
            Err(_) => {}
        }
    });

    PagedQuery { page, request }
}
