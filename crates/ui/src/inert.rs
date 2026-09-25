//! The one correct way for this console to set `inert`.
//!
//! `inert` is declared as an ordinary attribute rather than a boolean one, so a
//! Rust `bool` renders as `inert="true"` or `inert="false"` — and the browser
//! acts on the attribute's *presence*, not its value. `inert="false"` therefore
//! makes the element inert: no descendant can be focused, clicked, or found by
//! assistive technology, which is the exact opposite of what the flag means.
//!
//! The bug this replaces was silent in every test the console had, because an
//! inert element is still rendered, still visible, and still reports its text
//! through `textContent`. It showed up only as the page's own heading
//! disappearing from the accessibility tree while no dialog was open.

/// The value to pass for an `inert` attribute.
///
/// Present when `active`, omitted otherwise, so a page that is not showing a
/// modal stays fully interactive and fully announced.
pub fn attr(active: bool) -> Option<bool> {
    active.then_some(true)
}

#[cfg(test)]
mod tests {
    use super::attr;

    #[test]
    fn the_attribute_is_present_only_while_it_should_be() {
        assert_eq!(attr(true), Some(true));
        assert_eq!(attr(false), None);
    }
}
