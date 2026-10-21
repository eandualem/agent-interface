//! The tmux control-mode protocol and connections: the decoder, commands,
//! session and pane identity, and the connection lifecycle.
//!
//! This crate depends on no other project crate. Protocol parsing stays
//! separate from the transport that starts and reads processes. See
//! `docs/decisions/0003-control-mode-connection-and-attachment-model.md`.

#![forbid(unsafe_code)]

pub mod capture;
pub mod command;
pub mod control;
pub mod discovery;
pub mod layout;
pub mod recovery;

pub mod connection;

// Not activated by the desktop app until interoperability fixtures pass.
pub mod shells;
