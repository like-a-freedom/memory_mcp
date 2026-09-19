//! Local admin authentication pages.

use dioxus::prelude::*;

/// Admin login page.
#[component]
pub fn AdminLoginPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut loading = use_signal(|| false);

    rsx! {
        div { class: "container",
            h1 { "Admin Login" }
            if !error.read().is_empty() {
                div { class: "error", "{error.read()}" }
            }
            form { onsubmit: move |evt| {
                evt.prevent_default();
                // TODO: call admin auth API
            },
                div { class: "field",
                    label { "Username" }
                    input {
                        r#type: "text",
                        value: "{username}",
                        oninput: move |evt| username.set(evt.value()),
                    }
                }
                div { class: "field",
                    label { "Password" }
                    input {
                        r#type: "password",
                        value: "{password}",
                        oninput: move |evt| password.set(evt.value()),
                    }
                }
                button {
                    r#type: "submit",
                    disabled: *loading.read(),
                    if *loading.read() { "Logging in..." } else { "Login" }
                }
            }
        }
    }
}

/// Admin activation page (finish challenge with password).
#[component]
pub fn AdminActivationPage() -> Element {
    let code = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut success = use_signal(|| false);

    rsx! {
        div { class: "container",
            h1 { "Set Admin Password" }
            if *success.read() {
                div { class: "success",
                    p { "Password set successfully!" }
                    a { href: "/admin/login", "Go to Login" }
                }
            } else {
                if !error.read().is_empty() {
                    div { class: "error", "{error.read()}" }
                }
                form { onsubmit: move |evt| {
                    evt.prevent_default();
                    if *password.read() != *confirm_password.read() {
                        error.set("Passwords do not match".into());
                        return;
                    }
                    // TODO: call finish challenge API
                },
                    div { class: "field",
                        label { "New Password" }
                        input {
                            r#type: "password",
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
                        }
                    }
                    div { class: "field",
                        label { "Confirm Password" }
                        input {
                            r#type: "password",
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                        }
                    }
                    button { r#type: "submit", "Set Password" }
                }
            }
        }
    }
}

/// Admin reset page (finish reset challenge with new password).
#[component]
pub fn AdminResetPage() -> Element {
    let code = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut confirm_password = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut success = use_signal(|| false);

    rsx! {
        div { class: "container",
            h1 { "Reset Admin Password" }
            if *success.read() {
                div { class: "success",
                    p { "Password reset successfully!" }
                    a { href: "/admin/login", "Go to Login" }
                }
            } else {
                if !error.read().is_empty() {
                    div { class: "error", "{error.read()}" }
                }
                form { onsubmit: move |evt| {
                    evt.prevent_default();
                    if *password.read() != *confirm_password.read() {
                        error.set("Passwords do not match".into());
                        return;
                    }
                    // TODO: call finish challenge API
                },
                    div { class: "field",
                        label { "New Password" }
                        input {
                            r#type: "password",
                            value: "{password}",
                            oninput: move |evt| password.set(evt.value()),
                        }
                    }
                    div { class: "field",
                        label { "Confirm Password" }
                        input {
                            r#type: "password",
                            value: "{confirm_password}",
                            oninput: move |evt| confirm_password.set(evt.value()),
                        }
                    }
                    button { r#type: "submit", "Reset Password" }
                }
            }
        }
    }
}

/// Admin reauth dialog (reauthenticate with password).
#[component]
pub fn AdminReauthDialog() -> Element {
    let mut password = use_signal(String::new);
    let mut error = use_signal(String::new);

    rsx! {
        div { class: "dialog",
            h2 { "Reauthenticate" }
            p { "Your session requires recent authentication." }
            if !error.read().is_empty() {
                div { class: "error", "{error.read()}" }
            }
            form { onsubmit: move |evt| {
                evt.prevent_default();
                // TODO: call reauth API
            },
                div { class: "field",
                    label { "Password" }
                    input {
                        r#type: "password",
                        value: "{password}",
                        oninput: move |evt| password.set(evt.value()),
                    }
                }
                button { r#type: "submit", "Reauthenticate" }
            }
        }
    }
}
