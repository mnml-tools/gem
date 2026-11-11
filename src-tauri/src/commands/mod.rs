//! Tauri command handlers. Commands are thin: they resolve the session or database, run a
//! service and map errors to [`crate::error::AppError`].

pub mod app;
pub mod editor;
pub mod files;
pub mod sessions;
pub mod shell;
pub mod sites;
pub mod support;
pub mod transfers;

#[cfg(test)]
mod tests;
