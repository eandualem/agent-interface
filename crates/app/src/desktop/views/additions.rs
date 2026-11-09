//! Add a person's own skill folder or HTML document to the pane's profile.

use std::{io::Read, path::Path};

use agent_interface_roster::{
    profiles::{AddError, Connection, Error},
    views::{DOCUMENT_HTML_LIMIT, ReportQuery},
};
use gpui::{Context, Div, PathPromptOptions, Stateful, div, prelude::*, px};

use super::ViewPane;
use crate::desktop::{
    components as ui,
    design::{colors, tokens},
};

/// The provider refuses a title longer than this.
const TITLE_LIMIT: usize = 240;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Skill,
    Document,
}

#[derive(Default)]
pub(super) struct State {
    kind: Option<Kind>,
    running: bool,
    outcome: Option<Result<String, String>>,
}

/// Shown only on a profile connection, whose provider accepts these additions.
pub(super) fn button(
    pane: &ViewPane,
    kind: Kind,
    cx: &mut Context<ViewPane>,
) -> Option<Stateful<Div>> {
    pane.connection.as_ref()?;
    let (id, label) = match kind {
        Kind::Skill => ("add-skill", "Add skill…"),
        Kind::Document => ("add-document", "Add document…"),
    };
    Some(
        ui::segment(id, label, false)
            .on_click(cx.listener(move |pane, _, _, cx| choose(pane, kind, cx))),
    )
}

/// What became of the last addition of this kind.
pub(super) fn note(pane: &ViewPane, kind: Kind) -> Option<Div> {
    if pane.additions.kind != Some(kind) {
        return None;
    }
    let (t, c) = (tokens(), colors());
    let (text, color) = match &pane.additions.outcome {
        None if pane.additions.running => ("Adding…", c.muted),
        None => return None,
        Some(Ok(text)) => (text.as_str(), c.muted),
        Some(Err(text)) => (text.as_str(), c.attention_text),
    };
    Some(
        div()
            .flex_shrink_0()
            .px(px(t.space.s20))
            .pt(px(t.space.s8))
            .text_color(color)
            .child(text.to_owned()),
    )
}

fn choose(pane: &mut ViewPane, kind: Kind, cx: &mut Context<ViewPane>) {
    if pane.additions.running {
        return;
    }
    let selection = cx.prompt_for_paths(PathPromptOptions {
        files: kind == Kind::Document,
        directories: kind == Kind::Skill,
        multiple: false,
        prompt: Some("Add".into()),
    });
    cx.spawn(async move |this, cx| {
        if let Ok(Ok(Some(paths))) = selection.await
            && let Some(path) = paths.into_iter().next()
        {
            let _ = this.update(cx, |pane, cx| add(pane, kind, path, cx));
        }
    })
    .detach();
}

fn add(pane: &mut ViewPane, kind: Kind, path: std::path::PathBuf, cx: &mut Context<ViewPane>) {
    let Some(connection) = pane.connection.clone() else {
        return;
    };
    if pane.additions.running {
        return;
    }
    pane.additions = State {
        kind: Some(kind),
        running: true,
        outcome: None,
    };
    let work = cx.background_executor().spawn(async move {
        match kind {
            Kind::Skill => add_skill(&connection, &path),
            Kind::Document => add_document(&connection, &path),
        }
    });
    cx.spawn(async move |this, cx| {
        let outcome = work.await;
        let _ = this.update(cx, |pane, cx| {
            pane.additions.running = false;
            pane.additions.outcome = Some(outcome);
            // The new document is on the first page of all authors.
            if kind == Kind::Document && matches!(pane.reports.query, ReportQuery::Documents(..)) {
                pane.reports.query = ReportQuery::Documents(None, None);
                pane.reports.agent = None;
                pane.reports.previous.clear();
            }
            pane.refetch(false, cx);
        });
    })
    .detach();
    cx.notify();
}

fn add_skill(connection: &Connection, path: &Path) -> Result<String, String> {
    let added = connection.add_skill(path).map_err(refusal)?;
    let mut text = format!("Added {} for every agent in this profile.", added.name);
    for warning in added.warnings {
        text.push(' ');
        text.push_str(&warning);
    }
    Ok(text)
}

fn add_document(connection: &Connection, path: &Path) -> Result<String, String> {
    let title = title(path)?;
    let html = html(path)?;
    connection
        .add_document(&title, &html)
        .map_err(refusal)
        .map(|_| format!("Added {title}."))
}

/// The file's name is the title; there is no form to ask for another.
fn title(path: &Path) -> Result<String, String> {
    let html = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("html") || extension.eq_ignore_ascii_case("htm")
        });
    let stem = path.file_stem().and_then(|stem| stem.to_str());
    match stem {
        Some(stem) if html => Ok(stem.chars().take(TITLE_LIMIT).collect()),
        _ => Err("Choose an HTML file.".to_owned()),
    }
}

fn html(path: &Path) -> Result<String, String> {
    let unreadable = |_| "This file could not be read.".to_owned();
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(unreadable)?
        .take(DOCUMENT_HTML_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    if bytes.len() > DOCUMENT_HTML_LIMIT {
        return Err(format!(
            "This file is over the {} MB document limit.",
            DOCUMENT_HTML_LIMIT / (1024 * 1024)
        ));
    }
    String::from_utf8(bytes).map_err(|_| "This file is not UTF-8 text.".to_owned())
}

fn refusal(error: AddError) -> String {
    match error {
        AddError::Refused(reason) => reason,
        AddError::Connection(Error::Refused(_)) => "The profile refused this addition.".to_owned(),
        AddError::Connection(_) => "The workspace profile connection is unavailable.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_needs_an_html_file_within_the_limit() {
        assert_eq!(title(Path::new("/a/Overview.HTML")).unwrap(), "Overview");
        assert!(title(Path::new("/a/notes.txt")).is_err());
        assert!(title(Path::new("/a/no-extension")).is_err());
        let long = format!("/a/{}.html", "x".repeat(TITLE_LIMIT + 5));
        assert_eq!(title(Path::new(&long)).unwrap().len(), TITLE_LIMIT);

        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("a.html");
        std::fs::write(&file, "<h1>é</h1>").unwrap();
        assert_eq!(html(&file).unwrap(), "<h1>é</h1>");
        std::fs::write(&file, [0xff]).unwrap();
        assert!(html(&file).unwrap_err().contains("UTF-8"));
        std::fs::write(&file, vec![b'a'; DOCUMENT_HTML_LIMIT + 1]).unwrap();
        assert!(html(&file).unwrap_err().contains("limit"));
    }
}
