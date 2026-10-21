//! Pure reconciliation of one window's edits with a newer shared document.

use std::path::Path;
use tmux_workspaces_core::{Layout, merge::merge_shared_layout};

use crate::{Document, Error, json, project, schema};

/// A replacement arrangement plus the actions required by its caller.
#[derive(Debug)]
#[must_use = "apply the document and handle conflicts and input-focus changes"]
pub struct Reconciliation {
    /// Merged edits, or the latest shared arrangement after a conflict, with this
    /// window's surviving navigation restored. Pure reconciliation does not save;
    /// `Store::save` commits this document only when `conflict` is false.
    pub document: Document,
    /// The caller must tell the user that the conflicting edit was refreshed and
    /// needs to be retried. Independent local edits are also discarded on conflict,
    /// matching the Python tool's refresh behavior.
    pub conflict: bool,
    /// Move input focus to navigation and require a new pane selection before
    /// typing: the active tree or attachment changed, or the active tab disappeared.
    /// Saved working-directory metadata alone does not require an input handoff.
    pub active_tree_changed: bool,
}

impl Document {
    /// Reconcile `local` edits based on `self` with `latest`, without database I/O.
    /// Independent edits merge; conflicting or schema-invalid combinations refresh
    /// to `latest` and report `conflict`. Invalid local edits or an oversized result
    /// return an error without changing any input. Selections remain window-local.
    ///
    /// After applying the result, the caller uses `latest` as the next merge base,
    /// retaining the returned model as its local edits. A merge preserves JSON values
    /// but may normalize formatting and numeric spellings. `path` is diagnostic only.
    pub fn reconcile(
        &self,
        local: &Layout,
        latest: &Document,
        path: &Path,
    ) -> Result<Reconciliation, Error> {
        let fail = |detail| Error {
            path: path.to_owned(),
            detail,
        };
        let mine = self.with_model(local, path)?;
        let decode = |document: &Document| {
            json::decode(document.raw_json()).map_err(|error| fail(error.to_string()))
        };
        let merged = merge_shared_layout(&decode(self)?, &decode(&mine)?, &decode(latest)?);
        let candidate = match merged {
            Ok(value) => {
                let raw = serde_json::to_string(&value).map_err(|error| fail(error.to_string()))?;
                // The merged combination can exceed container limits or repeat an
                // identity even though each input document was valid by itself.
                json::decode(&raw)
                    .map_err(|error| error.to_string())
                    .and_then(|value| schema::shared_layout(&value))
                    .ok()
                    .map(|model| (raw, model))
            }
            Err(_) => None,
        };
        let conflict = candidate.is_none();
        let (raw, mut model) =
            candidate.unwrap_or_else(|| (latest.raw.clone(), latest.model.clone()));
        let active_tree_changed = model.retain_navigation(local);
        let document = if conflict {
            latest.with_model(&model, path)?
        } else {
            let raw = project::apply(&raw, &model).map_err(fail)?;
            Document::parse(raw, path)?
        };
        Ok(Reconciliation {
            document,
            conflict,
            active_tree_changed,
        })
    }
}
