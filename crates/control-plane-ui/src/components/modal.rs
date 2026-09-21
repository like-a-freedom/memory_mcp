//! The console's modal frame.
//!
//! This is a native `<dialog>` opened with the `open` attribute rather than
//! `showModal()`. That is a deliberate constraint, not an oversight: the
//! shipped CSP allows WebAssembly compilation and nothing else, so the console
//! cannot run a script to open a dialog, and a refused dynamic call is a
//! WebAssembly trap rather than a recoverable error. The `open` attribute needs
//! no script at all.
//!
//! The frame owns the three things every modal here has to get right — an
//! accessible name, a description, and Escape — so a caller cannot forget one.
//! While a modal is mounted the caller marks the page content `inert`, which is
//! what keeps focus inside it; the browser does that itself for `showModal()`
//! but not for `open`.
//!
//! **Escape is handled only while focus is inside the frame**, and a frame opened
//! with the `open` attribute is not focused by the browser: `autofocus` on the
//! frame's first control is honoured when the frame is inserted into a document
//! that keeps its focus, and ignored when the element the operator was using is
//! replaced in the same update — which is exactly what the re-authentication
//! frame does. So the frame's own controls, not Escape, are the guaranteed way
//! out, and every frame here renders a visible one. Moving focus explicitly
//! needs `HTMLElement.focus()`, which would mean naming two more `web-sys`
//! features in this crate's manifest.

use dioxus::prelude::*;

/// The two modal shapes this console has.
///
/// Each carries both its ARIA role and its panel styling, because they are the
/// same decision twice: a question can wait for an answer, a one-time secret
/// cannot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModalKind {
    /// A question the operator answers, in the ordinary panel.
    Question,
    /// Something the operator must deal with before leaving, in the panel used
    /// for a secret that will never be shown again.
    Alert,
}

impl ModalKind {
    const fn role(self) -> &'static str {
        match self {
            Self::Question => "dialog",
            Self::Alert => "alertdialog",
        }
    }

    const fn panel(self) -> &'static str {
        match self {
            Self::Question => "dialog",
            Self::Alert => "secret",
        }
    }
}

/// A modal panel with an accessible name, a description and Escape handling.
#[component]
pub fn Modal(
    /// Base for the frame's element ids: `{id}-title` and `{id}-description`.
    ///
    /// Explicit rather than generated so a test can name the panel it is
    /// waiting for, and so a page never depends on render order for its ids.
    id: String,
    kind: ModalKind,
    /// The frame's accessible name.
    title: String,
    /// One sentence saying why the panel is open.
    #[props(default)]
    description: Option<String>,
    /// Called for Escape and for the caller's own close control. Every modal
    /// must supply one, or Escape would silently do nothing.
    on_dismiss: EventHandler<()>,
    children: Element,
) -> Element {
    let title_id = format!("{id}-title");
    let description_id = format!("{id}-description");
    // An empty `aria-describedby` names nothing, which is the right outcome for a
    // modal that has no description: it must not point at an element that was
    // never rendered.
    let described_by = if description.is_some() {
        description_id.clone()
    } else {
        String::new()
    };
    let role = kind.role();
    let panel = kind.panel();

    rsx! {
        dialog {
            class: "modal-layer",
            open: true,
            role: "{role}",
            "aria-modal": "true",
            "aria-labelledby": "{title_id}",
            "aria-describedby": "{described_by}",
            tabindex: "-1",
            onkeydown: move |event: KeyboardEvent| {
                if event.key() == Key::Escape {
                    on_dismiss.call(());
                }
            },
            div { class: "{panel}",
                h2 { id: "{title_id}", "{title}" }
                if let Some(description) = description {
                    p { id: "{description_id}", class: "modal-description", "{description}" }
                }
                {children}
            }
        }
    }
}
