//! Sign-out for the OIDC account surface.
//!
//! The account pages (`/`, `/keys`, `/delete`) authenticate with a browser
//! cookie and read a fresh CSRF token per mutation, so they hold no session
//! token of their own. What they do share is the sign-out sequence and the copy
//! for a sign-out the server did not confirm, which used to be written out
//! separately on each page and drifted.

use dioxus::prelude::*;
use dioxus_router::Navigator;

use crate::api::ApiClient;
use crate::routes::Route;

/// Copy for a sign-out the console could not confirm.
///
/// The cookie may still be live, so the operator is told what is true and given
/// the two things they can actually do about it.
pub const SIGN_OUT_UNCONFIRMED: &str =
    "The console could not confirm sign-out. Close this tab, or try again.";

/// Revoke the account's browser session and return to the sign-in page.
///
/// `busy` is raised for the duration so the page can show progress and disable
/// the control; it is lowered again on failure so the retry is available. The
/// caller clears its own secrets before calling this.
pub fn end_account_session(
    navigator: Navigator,
    mut busy: Signal<bool>,
    mut error: Signal<Option<String>>,
) {
    busy.set(true);
    error.set(None);
    spawn(async move {
        match ApiClient::new("/".to_owned()).logout().await {
            Ok(()) => {
                navigator.replace(Route::Login {});
            }
            Err(_) => {
                busy.set(false);
                error.set(Some(SIGN_OUT_UNCONFIRMED.to_owned()));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::SIGN_OUT_UNCONFIRMED;

    #[test]
    fn the_unconfirmed_copy_states_the_uncertainty_and_the_way_out() {
        // This copy replaced a claim that the session had ended, which the
        // console cannot know after a transport failure.
        assert!(SIGN_OUT_UNCONFIRMED.contains("could not confirm"));
        assert!(SIGN_OUT_UNCONFIRMED.contains("try again"));
    }
}
