//! The tmux control-mode protocol and connections: the decoder, commands,
//! session and pane identity, and the connection lifecycle.
//!
//! This crate depends on no other project crate. Protocol parsing stays
//! separate from the transport that starts and reads processes. See
//! `docs/ARCHITECTURE.md`.

#![forbid(unsafe_code)]

pub mod capture;
pub mod command;
pub mod control;
pub mod discovery;
pub mod layout;
pub mod preflight;
pub mod recovery;

pub mod connection;

pub mod shells;
