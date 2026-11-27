//! The Access pane: what each agent may reach, who is above whom, and the
//! requests for more. Its model comes first; the interface follows.
// Used by the pane's interface, which is not drawn yet.
#![cfg_attr(not(test), allow(dead_code))]
mod model;
