//! Preserve file identities when a terminal receives a file paste or drop.

use gpui::{ClipboardEntry, ClipboardItem, ExternalPaths};

pub(super) fn paths(paths: &ExternalPaths) -> Option<String> {
    if paths.0.is_empty() {
        return None;
    }
    paths
        .0
        .iter()
        .map(|path| {
            let path = path.to_str()?;
            // Newlines/control bytes cannot be represented faithfully by the
            // terminal's paste encoder. Refuse instead of changing the filename.
            if path.chars().any(char::is_control) {
                return None;
            }
            Some(format!("'{}'", path.replace('\'', "'\\''")))
        })
        .collect::<Option<Vec<_>>>()
        .map(|paths| paths.join(" "))
}

pub(super) fn text(item: ClipboardItem) -> Option<String> {
    // Finder also offers plain text containing only the display names. Prefer
    // the actual paths, preserving spaces and keeping multiple files separate.
    if let Some(files) = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::ExternalPaths(paths) => Some(paths),
        _ => None,
    }) {
        return paths(files);
    }
    item.text()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copied_files_keep_full_quoted_paths_instead_of_display_names() {
        let files = ExternalPaths(
            [
                "/tmp/Screenshot at 12.37.25 PM.png".into(),
                "/tmp/reader's capture.png".into(),
            ]
            .into_iter()
            .collect(),
        );
        let mut item = ClipboardItem::new_string("Screenshot at 12.37.25 PM.png".into());
        item.entries
            .push(ClipboardEntry::ExternalPaths(files.clone()));
        let expected = "'/tmp/Screenshot at 12.37.25 PM.png' '/tmp/reader'\\''s capture.png'";
        assert_eq!(text(item).as_deref(), Some(expected));
        assert_eq!(paths(&files).as_deref(), Some(expected));
        assert_eq!(
            text(ClipboardItem::new_string("ordinary text".into())).as_deref(),
            Some("ordinary text")
        );
        assert!(
            paths(&ExternalPaths(
                ["/tmp/a\nb.png".into()].into_iter().collect()
            ))
            .is_none()
        );
    }
}
