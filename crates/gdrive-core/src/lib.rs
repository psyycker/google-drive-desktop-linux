//! Core of the gdrive-linux sync client.
//!
//! The sync model follows the "three trees" approach:
//! * the **remote tree** — a local mirror of Drive metadata, kept current via the Changes API,
//! * the **local tree** — the files on disk, observed with inotify plus periodic scans,
//! * the **synced tree** — the last state both sides agreed on.
//!
//! Every observed change only marks items dirty; the reconciler then compares the three
//! trees for each dirty item and decides what to do. That keeps the engine idempotent:
//! replaying an event, or missing one that a later scan picks up, never corrupts state.

pub mod api;
pub mod auth;
pub mod bandwidth;
pub mod config;
pub mod db;
pub mod engine;
pub mod gdoc;
pub mod ipc;
pub mod local;
pub mod status;
pub mod update;
