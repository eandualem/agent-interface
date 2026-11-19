//! Git diffs and code shown inline in a Document. The Document holds only a
//! pointer to commits; the app reads them from a local checkout and puts a
//! static view in place of the pointer before the page is served. A pointer
//! Git cannot answer stays exactly as authored. No GUI.

pub mod checkout;
pub mod git;
pub mod pointer;
