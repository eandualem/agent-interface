use caseless::default_case_fold_str;
use unicode_properties::{GeneralCategoryGroup, UnicodeGeneralCategory};

/// Attachment chooser state, independent of drawing, storage and tmux.
///
/// Names come from the caller's session observation. An offered name does not
/// establish availability. Before attaching, the caller must refresh the names,
/// check availability and apply the existing destination-pane guard.
#[derive(Debug, Default)]
pub struct SessionChoices {
    names: Vec<String>,
    rows: Vec<String>,
    query: String,
    index: usize,
    active: Option<String>,
    displayed: Option<String>,
    stale: bool,
}

impl SessionChoices {
    /// Replace session names, keeping selection by exact identity through a
    /// sorted insertion. If the selected name disappears, activation stays
    /// disarmed until explicit selection or a filter edit. No neighbour is chosen.
    pub fn update(&mut self, names: impl IntoIterator<Item = String>) {
        self.names = names.into_iter().collect();
        self.names.sort();
        self.names.dedup();
        self.refilter();
    }

    fn refilter(&mut self) {
        let query = default_case_fold_str(&self.query);
        self.rows = self
            .names
            .iter()
            .filter(|name| default_case_fold_str(name).contains(&query))
            .cloned()
            .collect();
        if self.rows.is_empty() {
            self.index = 0;
            self.active = None;
            self.displayed = None;
            self.stale = false;
            return;
        }
        if let Some(name) = &self.active {
            if let Some(index) = self.rows.iter().position(|row| row == name) {
                self.index = index;
            } else {
                self.stale = true;
            }
        }
        self.index = self.index.min(self.rows.len() - 1);
    }

    /// Current filter, including literal leading and trailing spaces.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Edit one character: Backspace/DEL remove a scalar, Ctrl-U clears, and
    /// printable characters append up to 80 scalars. Other controls do nothing.
    /// Accepted edits select the first match, which must be presented before
    /// activation. Matching uses full Unicode case folding, without normalization.
    pub fn edit(&mut self, key: char) {
        match key {
            '\u{15}' => self.query.clear(),
            '\u{7f}' | '\u{8}' => {
                self.query.pop();
            }
            ch if self.query.chars().count() < 80
                && (ch == ' '
                    || !matches!(
                        ch.general_category_group(),
                        GeneralCategoryGroup::Other | GeneralCategoryGroup::Separator
                    )) =>
            {
                self.query.push(ch);
            }
            _ => return,
        }
        self.index = 0;
        self.active = None;
        self.displayed = None;
        self.stale = false;
        self.refilter();
    }

    /// Filtered names in Python's case-sensitive name order.
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// Highlight position, or none for an empty list. A stale highlight is not
    /// an activatable choice; use [`Self::choice`] for activation.
    pub fn index(&self) -> Option<usize> {
        (!self.rows.is_empty()).then_some(self.index)
    }

    /// Select a loaded row by position. Invalid positions leave state unchanged.
    /// The caller must present the resulting highlight before activation.
    pub fn select(&mut self, index: usize) -> bool {
        let Some(name) = self.rows.get(index) else {
            return false;
        };
        self.index = index;
        self.active = Some(name.clone());
        self.displayed = None;
        self.stale = false;
        true
    }

    /// Move by rows, clamped at either end. Page and Home/End navigation can use
    /// larger offsets. An empty list does nothing.
    pub fn move_by(&mut self, offset: isize) {
        if !self.rows.is_empty() {
            self.select(
                self.index
                    .saturating_add_signed(offset)
                    .min(self.rows.len() - 1),
            );
        }
    }

    /// Record that the current highlight has actually been shown. Call only
    /// after presenting it, never merely while refreshing data for activation.
    /// A missing selected name remains stale even when its neighbour is shown.
    pub fn presented(&mut self) {
        if !self.stale {
            self.active = self.rows.get(self.index).cloned();
            self.displayed = self.active.clone();
        }
    }

    /// No choice is visible, for example because the chooser has been hidden.
    pub fn hide(&mut self) {
        self.displayed = None;
    }

    /// The shown selection if it is still offered; otherwise nothing. A newly
    /// arriving row cannot become a choice until it has been presented.
    pub fn choice(&self) -> Option<&str> {
        let name = self.active.as_deref()?;
        (!self.stale
            && self.displayed.as_deref() == Some(name)
            && self.rows.get(self.index).is_some_and(|row| row == name))
        .then_some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filtering_folds_unicode_preserves_spaces_and_bounds_edits() {
        let mut choices = SessionChoices::default();
        choices.update(["Straße", "STRASSE", "ΟΣ", "ß", "worker "].map(str::to_owned));
        for ch in "strasse".chars() {
            choices.edit(ch);
        }
        assert_eq!(choices.rows(), ["STRASSE", "Straße"]);
        assert_eq!(choices.choice(), None);
        choices.presented();
        assert_eq!(choices.choice(), Some("STRASSE"));
        choices.move_by(1);
        choices.presented();
        choices.edit('\u{8}');
        assert_eq!(choices.index(), Some(0));
        choices.edit('\u{15}');
        for ch in "ος".chars() {
            choices.edit(ch);
        }
        assert_eq!(choices.rows(), ["ΟΣ"]);
        choices.edit('\u{15}');
        choices.edit(' ');
        assert_eq!(choices.rows(), ["worker "]);
        choices.edit('\n');
        choices.edit('\u{a0}');
        assert_eq!(choices.query(), " ");
        choices.edit('\u{7f}');
        for _ in 0..81 {
            choices.edit('é');
        }
        assert_eq!(choices.query().chars().count(), 80);
        assert_eq!(choices.choice(), None);
        choices.edit('\u{7f}');
        assert_eq!(choices.query().chars().count(), 79);
    }

    #[test]
    fn refresh_keeps_identity_and_never_activates_a_replacement() {
        let mut choices = SessionChoices::default();
        choices.update(["beta", "delta"].map(str::to_owned));
        choices.presented();
        choices.move_by(1);
        choices.presented();
        choices.update(["alpha", "beta", "delta"].map(str::to_owned));
        assert_eq!(choices.index(), Some(2));
        assert_eq!(choices.choice(), Some("delta"));
        choices.update(["alpha", "beta"].map(str::to_owned));
        choices.presented();
        assert_eq!(choices.choice(), None);
        assert!(!choices.select(2));
        assert_eq!(choices.choice(), None);
        choices.move_by(isize::MIN);
        assert_eq!(choices.choice(), None);
        choices.presented();
        assert_eq!(choices.choice(), Some("alpha"));
        choices.move_by(isize::MAX);
        choices.presented();
        assert_eq!(choices.choice(), Some("beta"));
        choices.hide();
        assert_eq!(choices.choice(), None);
    }

    #[test]
    fn empty_results_and_unseen_arrivals_cannot_activate() {
        let mut choices = SessionChoices::default();
        choices.presented();
        choices.update(["worker".into()]);
        assert_eq!(choices.choice(), None);
        choices.presented();
        assert_eq!(choices.choice(), Some("worker"));
        choices.edit('z');
        choices.move_by(1);
        assert_eq!(choices.index(), None);
        assert_eq!(choices.choice(), None);
        choices.update(["zebra".into()]);
        assert_eq!(choices.choice(), None);
        choices.presented();
        assert_eq!(choices.choice(), Some("zebra"));
        choices.update([]);
        choices.update(["zebra".into()]);
        assert_eq!(choices.choice(), None);
    }
}
