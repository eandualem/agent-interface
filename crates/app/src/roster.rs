//! Explicitly started, read-only roster polling, independent of tmux discovery.

use agent_interface_roster::views;

mod bound;
pub use bound::{BoundRosterSource, BoundRosterUpdate};
pub(crate) mod content;
pub use content::{BoundContentSource, BoundContentUpdate};

/// The newest data for one Backbone view, or why there is none, with the
/// request it answers.
pub type ViewUpdate = (views::Request, Result<views::Data, &'static str>);
