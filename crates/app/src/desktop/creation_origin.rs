//! Local creation callbacks refuse selection or connection changes before saving.

use super::{Browser, layout_edit::Selection, navigation_origin::Origin};

#[cfg(test)]
mod tests;

#[derive(Clone)]
pub(super) struct Target {
    workspace: String,
    origin: Origin,
    selection: Selection,
}

impl Browser {
    pub(super) fn creation_target(&self) -> Option<Target> {
        let model = self.layout.as_ref()?.model();
        let workspace = model.workspace()?.id.clone();
        Some(Target {
            origin: self.navigation_origin(&workspace)?,
            workspace,
            selection: Selection::capture(model),
        })
    }

    pub(super) fn creation_is_current(&self, target: &Target) -> bool {
        self.navigation_origin(&target.workspace).as_ref() == Some(&target.origin)
            && self
                .layout
                .as_ref()
                .is_some_and(|layout| target.selection.unchanged(layout.model()))
    }
}
