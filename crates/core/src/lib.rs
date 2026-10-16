//! The workspace, tab and layout model of tmux-workspaces: stable identities,
//! validation, the shared-layout merge and local navigation.
//!
//! This crate depends on no other project crate, and holds no user-interface
//! types, runtime handles, database connections or terminal engine pointers.
//! See `docs/decisions/0006-workspace-and-crate-layout.md`.

#![forbid(unsafe_code)]
