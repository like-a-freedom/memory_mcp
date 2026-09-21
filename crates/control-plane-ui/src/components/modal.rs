//! The console's modal frame.
//!
//! This is a native `<dialog>` opened with the `open` attribute rather than
//! `showModal()`. That is a deliberate constraint, not an oversight: the
//! shipped CSP allows WebAssembly compilation and nothing else, so the console
//! cannot run a script to open a dialog, and a refused dynamic call is a
//! WebAssembly trap rather than a recoverable error. The `open` attribute needs
//! no script at all.
//!
//! The frame owns the four things every modal here has to get right — an
//! accessible name, a description, focus on open, and Escape — so a caller
//! cannot forget one. While a modal is mounted the caller marks the page content
//! `inert`, which is what keeps focus inside it; the browser does that itself for
//! `showModal()` but not for `open`.
//!
//! Escape is handled by the frame, and it reaches the frame only while focus is
//! inside it, so the frame puts focus there itself as it mounts. It has to: `open`
//! does not move focus the way `showModal()` does, and the control the operator
//! was using is made `inert` in the same update, which blurs focus to the
//! document body — a place from which a keypress never reaches this frame.
//!
//! That move is not left to the `autofocus` attribute, because the attribute
//! cannot carry the guarantee. Measured against the packaged console, the browser
//! processes `autofocus` in a task of its own, after this one, and skips it
//! altogether when the control the operator was using was removed in the same
//! update. The re-authentication panel removes exactly that control, so its
//! password field was never focused and Escape was dead there while the panel was
//! open. A panel that wants one of its own controls focused first asks for it with
//! [`claim_initial_focus`] instead, which the frame steps aside for; see
//! [`move_focus_inside`] for why the two cannot disagree.
//!
//! A frame that cannot be focused is still operable: its own controls are reached
//! by Tab and every frame here renders a visible one. So a refused focus is not
//! turned into an error the operator has to deal with.

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

/// Move focus into a frame that has just mounted, unless it is already there.
///
/// Escape is useless without this: `open` does not focus the frame the way
/// `showModal()` does, and the control the operator was using is made `inert` in
/// the same update, so focus falls to the document body and a keypress never
/// reaches the frame.
///
/// The move is conditional because the panel may already have taken focus for one
/// of its own controls through [`claim_initial_focus`]. Asking the document which
/// element holds focus is what tells the two apart, and it is what makes the two
/// helpers independent of the order their mount handlers run in: the last one to
/// run places focus, and the frame never pulls it back out of a panel that already
/// has focus inside.
///
/// A frame that cannot be identified cannot be asked, and doing nothing then
/// leaves the frame's own controls reachable by Tab.
fn move_focus_inside(mounted: &MountedData) {
    let Some(frame) = mounted.downcast::<web_sys::Element>() else {
        return;
    };
    // `contains` is true for the frame itself, so a frame that somehow already
    // holds focus is left alone as well.
    let already_inside = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.active_element())
        .is_some_and(|active| {
            let active: &web_sys::Node = &active;
            frame.contains(Some(active))
        });
    if already_inside {
        return;
    }
    // The renderer applies the focus while this call is being built, and hands
    // back a future that only carries the result of doing so; that future cannot
    // outlive this handler, because the mounted data is borrowed from the event,
    // so the result is dropped rather than awaited. The same holds below.
    drop(mounted.set_focus(true));
}

/// Take initial focus for one control inside a panel.
///
/// This is how a panel says "start here", and it replaces the `autofocus`
/// attribute, which is honoured only sometimes: the browser processes `autofocus`
/// in a task of its own, and skips it when the control the operator was using was
/// removed in the same update — the case the re-authentication panel is in. A
/// mount handler has no such condition, so the panel's intent and the frame's
/// Escape guarantee hold at once.
///
/// The control is focused whether this runs before or after the frame's own
/// [`move_focus_inside`]: running first, it is the reason that one finds focus
/// already inside and leaves it alone; running second, it simply takes focus from
/// the frame. Either way the control ends up focused and focus stays inside the
/// frame, which is what Escape needs.
pub fn claim_initial_focus(event: &MountedEvent) {
    drop(event.set_focus(true));
}

/// A modal panel with an accessible name, a description, focus and Escape
/// handling.
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
            onmounted: move |event: MountedEvent| move_focus_inside(&event),
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
