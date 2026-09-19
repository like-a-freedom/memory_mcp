//! Cursor pagination state shared by the client list and the key list.

use crate::admin_api::Page;

/// One page of a cursor-paginated collection plus the state of the pager.
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

/// How a fetched page relates to the page on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageDirection {
    /// First load, or an explicit refresh of the page already shown.
    Replace,
    /// The page after the current one.
    Next,
    /// The page before the current one.
    Previous,
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

    /// Mark a fetch as started without discarding the current rows.
    pub fn begin(&mut self) {
        self.loading = true;
        self.error = None;
    }

    /// Apply a successful fetch.
    pub fn accept(&mut self, page: Page<T>, cursor: Option<String>, direction: PageDirection) {
        match direction {
            PageDirection::Replace => self.previous.clear(),
            PageDirection::Next => self.previous.push(self.cursor.clone()),
            PageDirection::Previous => {
                self.previous.pop();
            }
        }
        self.items = page.items;
        self.next_cursor = page.next_cursor;
        self.cursor = cursor;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn page(items: Vec<&str>, next: Option<&str>) -> Page<String> {
        Page {
            items: items.into_iter().map(ToOwned::to_owned).collect(),
            next_cursor: next.map(ToOwned::to_owned),
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
            None,
            PageDirection::Replace,
        );
        assert!(paged.is_loaded());
        assert!(paged.has_next());
        assert_eq!(paged.cursor(), None);
        assert_eq!(paged.items(), ["a"]);
        assert_eq!(paged.next_cursor().as_deref(), Some("cursor-1"));

        // Forward.
        let cursor = paged.next_cursor();
        paged.accept(page(vec!["b"], None), cursor, PageDirection::Next);
        assert_eq!(paged.items(), ["b"]);
        assert!(paged.has_previous());
        assert!(!paged.has_next());
        assert_eq!(paged.cursor(), Some("cursor-1"));

        // Back.
        assert_eq!(paged.previous_cursor(), Some(None));
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            None,
            PageDirection::Previous,
        );
        assert_eq!(paged.items(), ["a"]);
        assert!(!paged.has_previous());
        assert_eq!(paged.cursor(), None);
    }

    #[test]
    fn a_failure_keeps_the_rows_already_on_screen() {
        let mut paged: Paged<String> = Paged::default();
        paged.accept(
            page(vec!["a"], Some("cursor-1")),
            None,
            PageDirection::Replace,
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
        paged.accept(page(Vec::new(), None), None, PageDirection::Replace);
        assert!(paged.is_loaded());
        assert!(paged.items().is_empty());
        assert!(paged.error().is_none());
    }
}
