//! The next-generation desktop process boundary.
//!
//! This crate intentionally does not depend on a GUI framework in its public
//! protocol. The launcher and viewer can therefore evolve independently.

pub mod credential_store;
pub mod frame_renderer;
pub mod i18n;
pub mod ipc;
pub mod launcher_store;
pub mod protocol;
pub mod smart_agent;
pub mod startup_log;
pub mod updater;
pub mod viewer_process;
pub mod windows_app;
