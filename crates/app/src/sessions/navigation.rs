use std::collections::HashMap;
use std::io;
use std::path::Path;

use agent_interface_core::Layout;

use crate::library::location::resolve_path;

/// Select an existing attachment on the window's source server.
///
/// Pass the window's captured source socket, not freshly read launcher context.
/// Saved references use their own socket; legacy references without one use the
/// window's source. Symlinks are resolved even when the socket is currently
/// absent. This reads paths only and never connects to or starts a server.
///
/// The layout must be validated and refreshed before selection. Recency counters
/// belong to this window. The caller still confirms the attachment after any
/// later refresh before handing off input. No match leaves navigation unchanged.
pub fn choose_attached_session(
    layout: &mut Layout,
    name: &str,
    tab_uses: &HashMap<String, u64>,
    window_source: &Path,
) -> io::Result<bool> {
    let source = resolve_path(window_source)?;
    Ok(
        layout.choose_attached_session(name, tab_uses, |saved| match saved {
            None => true,
            Some(path) => resolve_path(Path::new(path)).is_ok_and(|path| path == source),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_interface_core::{Attachment, Direction};

    #[test]
    fn saved_aliases_and_legacy_references_use_the_captured_source_without_connecting() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let alias = root.join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let source = root.join("absent socket");
        let mut layout = Layout::initial("/tmp".into());
        let legacy_pane = layout.pane().unwrap().id.clone();
        layout.attach(Some(Attachment {
            session: "worker with spaces".into(),
            source_socket: None,
        }));
        layout.split(Direction::Right, "/tmp".into(), true);
        layout.attach(Some(Attachment {
            session: "worker with spaces".into(),
            source_socket: Some(alias.join("absent socket").to_str().unwrap().into()),
        }));
        let saved_pane = layout.pane().unwrap().id.clone();
        let uses = HashMap::new();
        assert!(
            choose_attached_session(&mut layout, "worker with spaces", &uses, &source).unwrap()
        );
        assert_eq!(layout.pane().unwrap().id, saved_pane);

        // A different window source changes only the legacy reference's target.
        let other = root.join("other absent socket");
        assert!(choose_attached_session(&mut layout, "worker with spaces", &uses, &other).unwrap());
        assert_eq!(layout.pane().unwrap().id, legacy_pane);
        layout.attach(None);
        let before = layout.clone();
        assert!(
            !choose_attached_session(&mut layout, "worker with spaces", &uses, &other).unwrap()
        );
        assert_eq!(layout, before);
        assert!(
            choose_attached_session(&mut layout, "worker with spaces", &uses, &source).unwrap()
        );
        assert_eq!(layout.pane().unwrap().id, saved_pane);
        assert!(!source.exists() && !other.exists());
    }
}
