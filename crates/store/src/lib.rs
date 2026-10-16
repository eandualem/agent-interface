//! Access to `layouts.db`, the layout library shared with the Python
//! tmux-workspaces tool: schema-v2 decoding and encoding, and persistence
//! transactions.
//!
//! This crate may depend on `tmux-workspaces-core` only. See
//! `docs/decisions/0009-python-library-and-shell-server-compatibility.md`.

#![forbid(unsafe_code)]
