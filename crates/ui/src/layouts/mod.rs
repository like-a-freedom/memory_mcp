//! Route layouts and the application shell.
//!
//! A *layout* wraps a group of routes and persists across them; see
//! `console.rs` for the signed-in administrator console. `app.rs` is the shell
//! above the router, which every route renders inside.

pub mod app;
pub mod console;
