//! Query bookkeeping shared by the console's list and detail pages.
//!
//! Two ideas live here, and they are the same idea twice:
//!
//! * a *generation* names one deliberate request, so an answer that arrives
//!   after a newer request was issued is recognisably stale and can be dropped
//!   instead of overwriting fresher data;
//! * a *page* is the cursor stack a list walks through.
//!
//! The client list, the client detail page and the key list all need both, so
//! neither is reimplemented per page: one fix has to be found once, and a page
//! cannot drift from the others in how it treats a superseded answer.

use dioxus::prelude::*;

use crate::admin_api::Page;

/// Produce a fresh token for a request that must supersede an older one.
///
/// Wrapping is harmless: only equality with the generation a page currently
/// holds matters, so it would take 2^64 requests to collide.
pub const fn next_generation(current: u64) -> u64 {
    current.wrapping_add(1)
}

/// Advance a generation counter in place. See [`next_generation`].
pub fn bump_generation(generation: &mut Signal<u64>) {
    let next = next_generation(*generation.peek());
    generation.set(next);
}

/// How a fetched page relates to the page already on screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageDirection {
    /// First load, or an explicit refresh of the page already shown.
    #[default]
    Replace,
    /// The page after the current one.
    Next,
    /// The page before the current one.
    Previous,
}

/// One page of a cursor-paginated collection, plus the state of the pager.
#[derive(Clone, PartialEq)]
pub struct Paged<T> {
    /// Rows of the page currently shown.
    items: Vec<T>,
    /// Cursor that produced `items`; `None` is the first page.
    cursor: Option<String>,
    /// Cursor of the following page, when the backend reported one.
    next_cursor: Option<String>,
    /// Cursors of the pages before the current one, oldest first.
    previous: Vec<Option<String>>,
    loading: bool,
    loaded: bool,
    error: Option<String>,
}

impl<T> Default for Paged<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            cursor: None,
            next_cursor: None,
            previous: Vec::new(),
            loading: false,
            loaded: false,
            error: None,
        }
    }
}

impl<T> Paged<T> {
    pub fn items(&self) -> &[T] {
        &self.items
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// A page has been fetched, so an empty list is a real empty state rather
    /// than "not loaded yet".
    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn has_next(&self) -> bool {
        self.next_cursor.is_some()
    }

    pub fn has_previous(&self) -> bool {
        !self.previous.is_empty()
    }

    /// Which page of the walk is on screen, counting from one.
    ///
    /// The cursor API reports no total, so this is the position the operator
    /// reached rather than "page n of m". A refresh is the first page again,
    /// because it asks for the collection from the start.
    pub fn page_number(&self) -> usize {
        self.previous.len() + 1
    }

    /// The cursor of the page currently shown.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// Cursor of the following page, when there is one.
    pub fn next_cursor(&self) -> Option<String> {
        self.next_cursor.clone()
    }

    /// Cursor of the previous page, when there is one.
    pub fn previous_cursor(&self) -> Option<Option<String>> {
        self.previous.last().cloned()
    }

    /// Mark a fetch as started without discarding the rows already on screen.
    pub fn begin(&mut self) {
        self.loading = true;
        self.error = None;
    }

    /// Apply a successful fetch.
    ///
    /// Taking the cursor and direction together with the rows means the pager
    /// cannot be advanced by one request while the rows come from another.
    pub fn accept(&mut self, page: Page<T>, request: PageRequest) {
        match request.direction {
            PageDirection::Replace => self.previous.clear(),
            PageDirection::Next => self.previous.push(self.cursor.clone()),
            PageDirection::Previous => {
                self.previous.pop();
            }
        }
        self.items = page.items;
        self.next_cursor = page.next_cursor;
        self.cursor = request.cursor;
        self.loading = false;
        self.loaded = true;
        self.error = None;
    }

    /// Apply a failure, keeping the rows already on screen.
    pub fn fail(&mut self, message: String) {
        self.loading = false;
        self.error = Some(message);
    }
}

/// One in-flight page request, identified by its generation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageRequest {
    cursor: Option<String>,
    direction: PageDirection,
    generation: u64,
}

impl PageRequest {
    /// The cursor this request asks for; `None` is the first page.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }

    /// The token that identifies this request.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// A request for the page already on screen, for a poll.
    ///
    /// A poll re-reads what the operator is looking at, so it must not move the
    /// pager: the direction is always `Replace`, however the page was reached.
    /// Without this, a poll that lands after a `Next` would push the same cursor
    /// onto the back-stack a second time.
    pub fn refresh(cursor: Option<String>, generation: u64) -> Self {
        Self {
            cursor,
            direction: PageDirection::Replace,
            generation,
        }
    }
}

/// Ask for one page, superseding whatever request is in flight.
///
/// The page is put into its loading state here rather than by the caller, so
/// "a request was issued" and "a request is running" cannot disagree.
pub fn request_page<T: 'static>(
    request: &mut Signal<PageRequest>,
    mut page: Signal<Paged<T>>,
    cursor: Option<String>,
    direction: PageDirection,
) {
    page.write().begin();
    let generation = next_generation(request.peek().generation());
    request.set(PageRequest {
        cursor,
        direction,
        generation,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(items: Vec<&str>, next: Option<&str>) -> Page<String> {
        Page {
            items: items.into_iter().map(ToOwned::to_owned).collect(),
            next_cursor: next.map(ToOwned::to_owned),
        }
    }

    fn request(cursor: Option<&str>, direction: PageDirection) -> PageRequest {
        PageRequest {
            cursor: cursor.map(ToOwned::to_owned),
            direction,
            generation: 1,
        }
    }

    #[test]
    fn pager_moves_forward_and_back_without_losing_the_cursor_stack() {
        let mut paged: Paged<String> = Paged::default();
        assert!(!paged.is_loaded());
        assert!(!paged.has_next());
        assert!(!paged.has_previous());

        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            request(None, PageDirection::Replace),
        );
        assert!(paged.is_loaded());
        assert!(paged.has_next());
        assert_eq!(paged.cursor(), None);
        assert_eq!(paged.items(), ["a"]);
        assert_eq!(paged.next_cursor().as_deref(), Some("cursor-1"));

        // Forward.
        let cursor = paged.next_cursor();
        paged.accept(
            page(vec!["b"], None),
            request(cursor.as_deref(), PageDirection::Next),
        );
        assert_eq!(paged.items(), ["b"]);
        assert!(paged.has_previous());
        assert!(!paged.has_next());
        assert_eq!(paged.cursor(), Some("cursor-1"));

        // Back.
        assert_eq!(paged.previous_cursor(), Some(None));
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            request(None, PageDirection::Previous),
        );
        assert_eq!(paged.items(), ["a"]);
        assert!(!paged.has_previous());
        assert_eq!(paged.cursor(), None);
    }

    #[test]
    fn request_generations_wrap_without_panicking() {
        assert_eq!(next_generation(0), 1);
        assert_eq!(next_generation(u64::MAX), 0);
    }

    #[test]
    fn a_failure_keeps_the_rows_already_on_screen() {
        let mut paged: Paged<String> = Paged::default();
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            request(None, PageDirection::Replace),
        );
        paged.begin();
        assert!(paged.is_loading());
        paged.fail("The service is temporarily unavailable.".to_owned());
        assert_eq!(paged.items(), ["a"]);
        assert_eq!(
            paged.error(),
            Some("The service is temporarily unavailable.")
        );
        assert!(!paged.is_loading());
    }

    #[test]
    fn an_empty_page_is_loaded_rather_than_pending() {
        let mut paged: Paged<String> = Paged::default();
        assert!(!paged.is_loaded());
        paged.accept(
            page(Vec::new(), None),
            request(None, PageDirection::Replace),
        );
        assert!(paged.is_loaded());
        assert!(paged.items().is_empty());
        assert!(paged.error().is_none());
    }

    #[test]
    fn a_refresh_returns_to_the_first_page_even_when_the_operator_walked_forward() {
        let mut paged: Paged<String> = Paged::default();
        assert_eq!(paged.page_number(), 1);
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            request(None, PageDirection::Replace),
        );
        let cursor = paged.next_cursor();
        paged.accept(
            page(vec!["b"], None),
            request(cursor.as_deref(), PageDirection::Next),
        );
        assert_eq!(paged.page_number(), 2);
        // A refresh asks for the collection from the start, so the position the
        // operator had reached is gone with it.
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            request(None, PageDirection::Replace),
        );
        assert_eq!(paged.page_number(), 1);
    }

    #[test]
    fn a_request_defaults_to_the_first_page() {
        let request = PageRequest::default();
        assert_eq!(request.cursor(), None);
        assert_eq!(request.generation(), 0);
    }
}
