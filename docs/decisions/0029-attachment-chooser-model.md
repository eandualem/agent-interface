# 0029: Attachment chooser state and Unicode matching

Status: accepted for the non-UI model.

## Decision

Keep the attachment chooser's query, name ordering and selection in the core
model as `SessionChoices`. Session observations supply names; the model performs
no I/O and does not claim that a listed session is online. The controller must
refresh availability and apply the existing destination-pane guard before
attaching. The native chooser integration is described in [0028](0028-native-session-chooser.md).

Follow the Python attachment menu at
[`51de675`](https://github.com/eandualem/tmux-workspaces/tree/51de6757cd0aeda3c19fe68fa4ec5b93ab206804):
`sidebar.py` filters names with `str.casefold`, orders by the original name and
resets selection on filter edits. `menu.py` retains identity through sorted
insertions and disarms activation when the chosen entry disappears. Newly
arriving entries must be presented before activation. Movement clamps at the
ends; it does not use the separate new-pane chooser's wraparound behavior.

The renderer reports when a highlight was presented. Refreshing observations
alone cannot arm an unseen row. This model has no viewport or drawing policy;
those remain the controller's responsibility.

Use the exact dependency `caseless` 0.2.2, licensed MIT, for default full Unicode
case folding. Its data is Unicode 16.0.0. Use `default_case_fold_str` only, without
canonical or compatibility normalization: matching preserves accents and
literal spaces as Python does. The crate's `unicode-normalization` dependency
(`(MIT OR Apache-2.0) AND Unicode-3.0`), locked at 0.1.26, is also new to the
workspace. Filter edits use the
existing `unicode-properties` 0.1.4 general categories, as model names do.

## Alternatives and evidence

- ASCII or ordinary lowercase matching misses full folds, including sharp S to
  `ss` and final sigma to sigma. It does not match Python for Unicode names.
- Rust 1.99.1's `char::to_casefold_unnormalized` remains unstable. Changing the
  toolchain or enabling unstable standard-library features is unnecessary.
- A handwritten folding table adds Unicode maintenance to this project.
- A generic menu framework adds separators, callbacks and rendering policy
  that the attachment-name model does not need.

Sources: [caseless API](https://docs.rs/caseless/0.2.2/caseless/),
[crate source and licence](https://github.com/unicode-rs/rust-caseless),
[Rust case-fold API](https://doc.rust-lang.org/std/primitive.char.html#method.to_casefold_unnormalized).
Dependency metadata and source were checked on 21 October 2026. Unicode database
updates may add newly assigned characters; this version does not claim matches
for mappings introduced after Unicode 16.
