# Gate 5: control-mode fidelity on tmux 3.3a

**Passed.** Of 200 comparisons, 167 match, 33 differ as a limit of decision record 0021 explains and 0 differ unexplained. 0 fixture runs failed to complete. 0 of 31 checks failed.

## Environment

| | |
| --- | --- |
| revision | 1e475fd11247faae4672948f16c0742214864a94 |
| os | macOS 27.0.1 (26A434) |
| cpu | Apple M5, 10 cores (4 performance, 6 efficiency) |
| memory | 16 GiB |
| power | Now drawing from 'AC Power'; lowpowermode 0 |
| other builds running before | none |
| tmux | 3.3a |
| rustc | rustc 1.99.1 (58cf8f9c3 2026-10-13) |
| zig | 0.15.2 |
| build profile | release (opt-level 3, debug info false, debug assertions false) |
| libghostty-vt | 0.1.0-dev |
| libghostty-vt optimisation | ReleaseFast |
| libghostty-vt SIMD | true |
| libghostty-vt CPU target | baseline |
| LIBGHOSTTY_VT_SYS_OPTIMIZE | ReleaseFast |
| rio-vt | 0.5.28, default features off |
| alacritty_terminal | 0.26.0 |

## Comparisons

| Scenario | Fixture | Compared | Result |
| --- | --- | --- | --- |
| attach | styles | after the rebuild | match |
| attach | styles | after more output and input | match |
| attach | background-erase | after the rebuild | differs (75 cells): tmux 3.3 captures a row only up to its last written cell |
| attach | background-erase | after more output and input | differs (69 cells): tmux 3.3 captures a row only up to its last written cell |
| attach | wide | after the rebuild | match |
| attach | wide | after more output and input | match |
| attach | line-drawing | after the rebuild | match |
| attach | line-drawing | after more output and input | match |
| attach | history | after the rebuild | match |
| attach | history | after more output and input | match |
| attach | alternate | after the rebuild | match |
| attach | alternate | after more output and input | match |
| attach | pending-csi | after the rebuild | match |
| attach | pending-csi | after more output and input | match |
| attach | pending-csi-line-feed | after the rebuild | match |
| attach | pending-csi-line-feed | after more output and input | match |
| attach | pending-osc | after the rebuild | match |
| attach | pending-osc | after more output and input | match |
| attach | pending-wrap | after the rebuild | match |
| attach | pending-wrap | after more output and input | match |
| attach | wide-pending-wrap | after the rebuild | match |
| attach | wide-pending-wrap | after more output and input | match |
| attach | origin-no-wrap | after the rebuild | match |
| attach | origin-no-wrap | after more output and input | match |
| attach | links | after the rebuild | match |
| attach | links | after more output and input | match |
| attach | link-wrap | after the rebuild | match |
| attach | link-wrap | after more output and input | match |
| attach | tabs | after the rebuild | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| attach | tabs | after more output and input | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| attach | tab-moved | after the rebuild | match |
| attach | tab-moved | after more output and input | match |
| attach | underline-colours | after the rebuild | match |
| attach | underline-colours | after more output and input | match |
| attach | current-style | after the rebuild | match |
| attach | current-style | after more output and input | differs (17 cells): the current text style is not exported |
| attach | partial-utf8 | after the rebuild | match |
| attach | partial-utf8 | after more output and input | differs (11 cells): a partly received UTF-8 character is not exported |
| attach | saved-cursor | after the rebuild | match |
| attach | saved-cursor | after more output and input | differs (21 cells, cursor): the general saved cursor (DECSC) is not exported |
| attach | still-line-drawing | after the rebuild | match |
| attach | still-line-drawing | after more output and input | differs (2 cells): the character set in use is not exported |
| attach | screen-title | after the rebuild | match |
| attach | screen-title | after more output and input | match |
| attach | screen-title-pending | after the rebuild | match |
| attach | screen-title-pending | after more output and input | match |
| pause | styles | after the rebuild | match |
| pause | styles | after more output and input | match |
| pause | background-erase | after the rebuild | differs (75 cells): tmux 3.3 captures a row only up to its last written cell |
| pause | background-erase | after more output and input | differs (69 cells): tmux 3.3 captures a row only up to its last written cell |
| pause | wide | after the rebuild | match |
| pause | wide | after more output and input | match |
| pause | line-drawing | after the rebuild | match |
| pause | line-drawing | after more output and input | match |
| pause | history | after the rebuild | match |
| pause | history | after more output and input | match |
| pause | alternate | after the rebuild | match |
| pause | alternate | after more output and input | match |
| pause | pending-csi | after the rebuild | match |
| pause | pending-csi | after more output and input | match |
| pause | pending-csi-line-feed | after the rebuild | match |
| pause | pending-csi-line-feed | after more output and input | match |
| pause | pending-osc | after the rebuild | match |
| pause | pending-osc | after more output and input | match |
| pause | pending-wrap | after the rebuild | match |
| pause | pending-wrap | after more output and input | match |
| pause | wide-pending-wrap | after the rebuild | match |
| pause | wide-pending-wrap | after more output and input | match |
| pause | origin-no-wrap | after the rebuild | match |
| pause | origin-no-wrap | after more output and input | match |
| pause | links | after the rebuild | match |
| pause | links | after more output and input | match |
| pause | link-wrap | after the rebuild | match |
| pause | link-wrap | after more output and input | match |
| pause | tabs | after the rebuild | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| pause | tabs | after more output and input | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| pause | tab-moved | after the rebuild | match |
| pause | tab-moved | after more output and input | match |
| pause | underline-colours | after the rebuild | match |
| pause | underline-colours | after more output and input | match |
| pause | current-style | after the rebuild | match |
| pause | current-style | after more output and input | differs (17 cells): the current text style is not exported |
| pause | partial-utf8 | after the rebuild | match |
| pause | partial-utf8 | after more output and input | differs (11 cells): a partly received UTF-8 character is not exported |
| pause | saved-cursor | after the rebuild | match |
| pause | saved-cursor | after more output and input | differs (21 cells, cursor): the general saved cursor (DECSC) is not exported |
| pause | still-line-drawing | after the rebuild | match |
| pause | still-line-drawing | after more output and input | differs (2 cells): the character set in use is not exported |
| pause | screen-title | after the rebuild | match |
| pause | screen-title | after more output and input | match |
| pause | screen-title-pending | after the rebuild | match |
| pause | screen-title-pending | after more output and input | match |
| split-attach | styles | after the rebuild | match |
| split-attach | styles | after more output and input | match |
| split-attach | background-erase | after the rebuild | differs (75 cells): tmux 3.3 captures a row only up to its last written cell |
| split-attach | background-erase | after more output and input | differs (69 cells): tmux 3.3 captures a row only up to its last written cell |
| split-attach | line-drawing | after the rebuild | match |
| split-attach | line-drawing | after more output and input | match |
| split-attach | history | after the rebuild | match |
| split-attach | history | after more output and input | match |
| split-attach | alternate | after the rebuild | match |
| split-attach | alternate | after more output and input | match |
| split-attach | pending-csi | after the rebuild | match |
| split-attach | pending-csi | after more output and input | match |
| split-attach | pending-csi-line-feed | after the rebuild | match |
| split-attach | pending-csi-line-feed | after more output and input | match |
| split-attach | pending-osc | after the rebuild | match |
| split-attach | pending-osc | after more output and input | match |
| split-attach | pending-wrap | after the rebuild | match |
| split-attach | pending-wrap | after more output and input | match |
| split-attach | wide-pending-wrap | after the rebuild | match |
| split-attach | wide-pending-wrap | after more output and input | match |
| split-attach | origin-no-wrap | after the rebuild | match |
| split-attach | origin-no-wrap | after more output and input | match |
| split-attach | links | after the rebuild | match |
| split-attach | links | after more output and input | match |
| split-attach | link-wrap | after the rebuild | match |
| split-attach | link-wrap | after more output and input | match |
| split-attach | tabs | after the rebuild | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| split-attach | tabs | after more output and input | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| split-attach | tab-moved | after the rebuild | match |
| split-attach | tab-moved | after more output and input | match |
| split-attach | underline-colours | after the rebuild | match |
| split-attach | underline-colours | after more output and input | match |
| split-attach | current-style | after the rebuild | match |
| split-attach | current-style | after more output and input | differs (17 cells): the current text style is not exported |
| split-attach | partial-utf8 | after the rebuild | match |
| split-attach | partial-utf8 | after more output and input | differs (11 cells): a partly received UTF-8 character is not exported |
| split-attach | saved-cursor | after the rebuild | match |
| split-attach | saved-cursor | after more output and input | differs (21 cells, cursor): the general saved cursor (DECSC) is not exported |
| split-attach | still-line-drawing | after the rebuild | match |
| split-attach | still-line-drawing | after more output and input | differs (2 cells): the character set in use is not exported |
| split-attach | screen-title | after the rebuild | match |
| split-attach | screen-title | after more output and input | match |
| split-attach | screen-title-pending | after the rebuild | match |
| split-attach | screen-title-pending | after more output and input | match |
| split-pause | styles | after the rebuild | match |
| split-pause | styles | after more output and input | match |
| split-pause | background-erase | after the rebuild | differs (75 cells): tmux 3.3 captures a row only up to its last written cell |
| split-pause | background-erase | after more output and input | differs (69 cells): tmux 3.3 captures a row only up to its last written cell |
| split-pause | line-drawing | after the rebuild | match |
| split-pause | line-drawing | after more output and input | match |
| split-pause | history | after the rebuild | match |
| split-pause | history | after more output and input | match |
| split-pause | alternate | after the rebuild | match |
| split-pause | alternate | after more output and input | match |
| split-pause | pending-csi | after the rebuild | match |
| split-pause | pending-csi | after more output and input | match |
| split-pause | pending-csi-line-feed | after the rebuild | match |
| split-pause | pending-csi-line-feed | after more output and input | match |
| split-pause | pending-osc | after the rebuild | match |
| split-pause | pending-osc | after more output and input | match |
| split-pause | pending-wrap | after the rebuild | match |
| split-pause | pending-wrap | after more output and input | match |
| split-pause | wide-pending-wrap | after the rebuild | match |
| split-pause | wide-pending-wrap | after more output and input | match |
| split-pause | origin-no-wrap | after the rebuild | match |
| split-pause | origin-no-wrap | after more output and input | match |
| split-pause | links | after the rebuild | match |
| split-pause | links | after more output and input | match |
| split-pause | link-wrap | after the rebuild | match |
| split-pause | link-wrap | after more output and input | match |
| split-pause | tabs | after the rebuild | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| split-pause | tabs | after more output and input | differs (23 cells): tmux 3.3 captures a row only up to its last written cell |
| split-pause | tab-moved | after the rebuild | match |
| split-pause | tab-moved | after more output and input | match |
| split-pause | underline-colours | after the rebuild | match |
| split-pause | underline-colours | after more output and input | match |
| split-pause | current-style | after the rebuild | match |
| split-pause | current-style | after more output and input | differs (17 cells): the current text style is not exported |
| split-pause | partial-utf8 | after the rebuild | match |
| split-pause | partial-utf8 | after more output and input | differs (11 cells): a partly received UTF-8 character is not exported |
| split-pause | saved-cursor | after the rebuild | match |
| split-pause | saved-cursor | after more output and input | differs (21 cells, cursor): the general saved cursor (DECSC) is not exported |
| split-pause | still-line-drawing | after the rebuild | match |
| split-pause | still-line-drawing | after more output and input | differs (2 cells): the character set in use is not exported |
| split-pause | screen-title | after the rebuild | match |
| split-pause | screen-title | after more output and input | match |
| split-pause | screen-title-pending | after the rebuild | match |
| split-pause | screen-title-pending | after more output and input | match |
| resize | reflow | after the rebuild at 30x8 | match |
| resize | reflow | after the rebuild at 50x10 | match |
| resize | reflow | after the rebuild at 40x6 | match |
| resize | reflow | after the rebuild at 36x9 | match |
| resize | reflow | after more output and input | match |
| resize | alternate-resize | after the rebuild at 30x8 | match |
| resize | alternate-resize | after the rebuild at 50x10 | match |
| resize | alternate-resize | after the rebuild at 40x6 | match |
| resize | alternate-resize | after the rebuild at 36x9 | match |
| resize | alternate-resize | after more output and input | differs (36 cells): a capture returns the saved primary screen at the current width |
| resize | redraw | after the rebuild at 36x9 | match |
| resize | redraw | after more output and input | match |
| resize | stream | after the rebuild at 36x9 | match |
| resize | stream | after more output and input | match |
| flow | count | after the rebuild | match |
| flow | count | after more output and input | match |
| own-pause | count | after the rebuild | match |
| own-pause | count | after more output and input | match |
| exit | remaining pane | after the rebuild | match |
| exit | remaining pane | after more output and input | match |

## Checks

tmux's own pause: 20 of 20 stalls ended in a pause.

| Scenario | Check | Result |
| --- | --- | --- |
| flow | rebuild 1 continues the count | passed |
| flow | rebuild 2 continues the count | passed |
| flow | rebuild 3 continues the count | passed |
| flow | rebuild 4 continues the count | passed |
| flow | rebuild 5 continues the count | passed |
| flow | rebuild 6 continues the count | passed |
| own-pause | rebuild after pause 1 continues the count | passed |
| own-pause | rebuild after pause 2 continues the count | passed |
| own-pause | rebuild after pause 3 continues the count | passed |
| own-pause | rebuild after pause 4 continues the count | passed |
| own-pause | rebuild after pause 5 continues the count | passed |
| own-pause | rebuild after pause 6 continues the count | passed |
| own-pause | rebuild after pause 7 continues the count | passed |
| own-pause | rebuild after pause 8 continues the count | passed |
| own-pause | rebuild after pause 9 continues the count | passed |
| own-pause | rebuild after pause 10 continues the count | passed |
| own-pause | rebuild after pause 11 continues the count | passed |
| own-pause | rebuild after pause 12 continues the count | passed |
| own-pause | rebuild after pause 13 continues the count | passed |
| own-pause | rebuild after pause 14 continues the count | passed |
| own-pause | rebuild after pause 15 continues the count | passed |
| own-pause | rebuild after pause 16 continues the count | passed |
| own-pause | rebuild after pause 17 continues the count | passed |
| own-pause | rebuild after pause 18 continues the count | passed |
| own-pause | rebuild after pause 19 continues the count | passed |
| own-pause | rebuild after pause 20 continues the count | passed |
| exit | a pane that exits after its pause fails the rebuild | passed |
| exit | a pane that exits during a first attach fails the line, and a second line rebuilds the rest | passed |
| hooks | no hooks: not reported | passed |
| hooks | a session hook on display-message is reported | passed |
| hooks | a global hook on capture-pane is reported | passed |

## Not compared

- Modes tmux 3.3a does not report: bracketed paste, focus reporting.
- The extended-keys mode: the terminal engine does not report it.

## Differences

### attach / background-erase, after the rebuild: differs (75 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 2, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 3, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 4, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 5, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 6, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 69 more cells

### attach / background-erase, after more output and input: differs (69 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 8, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 9, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 10, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 11, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 12, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 63 more cells

### attach / tabs, after the rebuild: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### attach / tabs, after more output and input: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### attach / current-style, after more output and input: differs (17 cells): the current text style is not exported

- row 0, column 1, [Foreground, Background]: reference "a" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "a" Narrow
- row 0, column 2, [Foreground, Background]: reference "f" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "f" Narrow
- row 0, column 3, [Foreground, Background]: reference "t" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "t" Narrow
- row 0, column 4, [Foreground, Background]: reference "e" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "e" Narrow
- row 0, column 5, [Foreground, Background]: reference "r" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "r" Narrow
- row 0, column 6, [Foreground, Background]: reference "i" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "i" Narrow
- and 11 more cells

### attach / partial-utf8, after more output and input: differs (11 cells): a partly received UTF-8 character is not exported

- row 0, column 3, [Text, Width]: reference "界" Wide; rebuilt "�" Narrow
- row 0, column 4, [Text, Width]: reference "" Spacer; rebuilt "p" Narrow
- row 0, column 5, [Text]: reference "p" Narrow; rebuilt "o" Narrow
- row 0, column 6, [Text]: reference "o" Narrow; rebuilt "s" Narrow
- row 0, column 7, [Text]: reference "s" Narrow; rebuilt "t" Narrow
- row 0, column 8, [Text]: reference "t" Narrow; rebuilt "i" Narrow
- and 5 more cells

### attach / saved-cursor, after more output and input: differs (21 cells, cursor): the general saved cursor (DECSC) is not exported

- cursor at (7, 1), the reference's at (7, 2)
- row 0, column 0, [Text]: reference "" Narrow; rebuilt "X" Narrow
- row 0, column 1, [Text]: reference "" Narrow; rebuilt "i" Narrow
- row 0, column 2, [Text]: reference "" Narrow; rebuilt "n" Narrow
- row 0, column 3, [Text]: reference "" Narrow; rebuilt "p" Narrow
- row 0, column 4, [Text]: reference "" Narrow; rebuilt "u" Narrow
- row 0, column 5, [Text]: reference "" Narrow; rebuilt "t" Narrow
- and 15 more cells

### attach / still-line-drawing, after more output and input: differs (2 cells): the character set in use is not exported

- row 0, column 2, [Text]: reference "─" Narrow; rebuilt "q" Narrow
- row 0, column 3, [Text]: reference "┐" Narrow; rebuilt "k" Narrow

### pause / background-erase, after the rebuild: differs (75 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 2, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 3, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 4, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 5, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 6, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 69 more cells

### pause / background-erase, after more output and input: differs (69 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 8, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 9, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 10, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 11, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 12, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 63 more cells

### pause / tabs, after the rebuild: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### pause / tabs, after more output and input: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### pause / current-style, after more output and input: differs (17 cells): the current text style is not exported

- row 0, column 1, [Foreground, Background]: reference "a" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "a" Narrow
- row 0, column 2, [Foreground, Background]: reference "f" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "f" Narrow
- row 0, column 3, [Foreground, Background]: reference "t" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "t" Narrow
- row 0, column 4, [Foreground, Background]: reference "e" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "e" Narrow
- row 0, column 5, [Foreground, Background]: reference "r" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "r" Narrow
- row 0, column 6, [Foreground, Background]: reference "i" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "i" Narrow
- and 11 more cells

### pause / partial-utf8, after more output and input: differs (11 cells): a partly received UTF-8 character is not exported

- row 0, column 3, [Text, Width]: reference "界" Wide; rebuilt "�" Narrow
- row 0, column 4, [Text, Width]: reference "" Spacer; rebuilt "p" Narrow
- row 0, column 5, [Text]: reference "p" Narrow; rebuilt "o" Narrow
- row 0, column 6, [Text]: reference "o" Narrow; rebuilt "s" Narrow
- row 0, column 7, [Text]: reference "s" Narrow; rebuilt "t" Narrow
- row 0, column 8, [Text]: reference "t" Narrow; rebuilt "i" Narrow
- and 5 more cells

### pause / saved-cursor, after more output and input: differs (21 cells, cursor): the general saved cursor (DECSC) is not exported

- cursor at (7, 1), the reference's at (7, 2)
- row 0, column 0, [Text]: reference "" Narrow; rebuilt "X" Narrow
- row 0, column 1, [Text]: reference "" Narrow; rebuilt "i" Narrow
- row 0, column 2, [Text]: reference "" Narrow; rebuilt "n" Narrow
- row 0, column 3, [Text]: reference "" Narrow; rebuilt "p" Narrow
- row 0, column 4, [Text]: reference "" Narrow; rebuilt "u" Narrow
- row 0, column 5, [Text]: reference "" Narrow; rebuilt "t" Narrow
- and 15 more cells

### pause / still-line-drawing, after more output and input: differs (2 cells): the character set in use is not exported

- row 0, column 2, [Text]: reference "─" Narrow; rebuilt "q" Narrow
- row 0, column 3, [Text]: reference "┐" Narrow; rebuilt "k" Narrow

### split-attach / background-erase, after the rebuild: differs (75 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 2, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 3, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 4, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 5, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 6, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 69 more cells

### split-attach / background-erase, after more output and input: differs (69 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 8, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 9, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 10, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 11, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 12, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 63 more cells

### split-attach / tabs, after the rebuild: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### split-attach / tabs, after more output and input: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### split-attach / current-style, after more output and input: differs (17 cells): the current text style is not exported

- row 0, column 1, [Foreground, Background]: reference "a" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "a" Narrow
- row 0, column 2, [Foreground, Background]: reference "f" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "f" Narrow
- row 0, column 3, [Foreground, Background]: reference "t" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "t" Narrow
- row 0, column 4, [Foreground, Background]: reference "e" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "e" Narrow
- row 0, column 5, [Foreground, Background]: reference "r" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "r" Narrow
- row 0, column 6, [Foreground, Background]: reference "i" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "i" Narrow
- and 11 more cells

### split-attach / partial-utf8, after more output and input: differs (11 cells): a partly received UTF-8 character is not exported

- row 0, column 3, [Text, Width]: reference "界" Wide; rebuilt "�" Narrow
- row 0, column 4, [Text, Width]: reference "" Spacer; rebuilt "p" Narrow
- row 0, column 5, [Text]: reference "p" Narrow; rebuilt "o" Narrow
- row 0, column 6, [Text]: reference "o" Narrow; rebuilt "s" Narrow
- row 0, column 7, [Text]: reference "s" Narrow; rebuilt "t" Narrow
- row 0, column 8, [Text]: reference "t" Narrow; rebuilt "i" Narrow
- and 5 more cells

### split-attach / saved-cursor, after more output and input: differs (21 cells, cursor): the general saved cursor (DECSC) is not exported

- cursor at (7, 1), the reference's at (7, 2)
- row 0, column 0, [Text]: reference "" Narrow; rebuilt "X" Narrow
- row 0, column 1, [Text]: reference "" Narrow; rebuilt "i" Narrow
- row 0, column 2, [Text]: reference "" Narrow; rebuilt "n" Narrow
- row 0, column 3, [Text]: reference "" Narrow; rebuilt "p" Narrow
- row 0, column 4, [Text]: reference "" Narrow; rebuilt "u" Narrow
- row 0, column 5, [Text]: reference "" Narrow; rebuilt "t" Narrow
- and 15 more cells

### split-attach / still-line-drawing, after more output and input: differs (2 cells): the character set in use is not exported

- row 0, column 2, [Text]: reference "─" Narrow; rebuilt "q" Narrow
- row 0, column 3, [Text]: reference "┐" Narrow; rebuilt "k" Narrow

### split-pause / background-erase, after the rebuild: differs (75 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 2, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 3, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 4, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 5, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 6, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 69 more cells

### split-pause / background-erase, after more output and input: differs (69 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 1, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 8, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 9, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 10, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 11, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 12, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 63 more cells

### split-pause / tabs, after the rebuild: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### split-pause / tabs, after more output and input: differs (23 cells): tmux 3.3 captures a row only up to its last written cell

- row 0, column 17, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 18, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 19, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 20, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 21, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- row 0, column 22, [Background]: reference "" Narrow Style { foreground: Default, background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "" Narrow
- and 17 more cells

### split-pause / current-style, after more output and input: differs (17 cells): the current text style is not exported

- row 0, column 1, [Foreground, Background]: reference "a" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "a" Narrow
- row 0, column 2, [Foreground, Background]: reference "f" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "f" Narrow
- row 0, column 3, [Foreground, Background]: reference "t" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "t" Narrow
- row 0, column 4, [Foreground, Background]: reference "e" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "e" Narrow
- row 0, column 5, [Foreground, Background]: reference "r" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "r" Narrow
- row 0, column 6, [Foreground, Background]: reference "i" Narrow Style { foreground: Palette(1), background: Palette(4), underline: None, attributes: Attributes(0) }; rebuilt "i" Narrow
- and 11 more cells

### split-pause / partial-utf8, after more output and input: differs (11 cells): a partly received UTF-8 character is not exported

- row 0, column 3, [Text, Width]: reference "界" Wide; rebuilt "�" Narrow
- row 0, column 4, [Text, Width]: reference "" Spacer; rebuilt "p" Narrow
- row 0, column 5, [Text]: reference "p" Narrow; rebuilt "o" Narrow
- row 0, column 6, [Text]: reference "o" Narrow; rebuilt "s" Narrow
- row 0, column 7, [Text]: reference "s" Narrow; rebuilt "t" Narrow
- row 0, column 8, [Text]: reference "t" Narrow; rebuilt "i" Narrow
- and 5 more cells

### split-pause / saved-cursor, after more output and input: differs (21 cells, cursor): the general saved cursor (DECSC) is not exported

- cursor at (7, 1), the reference's at (7, 2)
- row 0, column 0, [Text]: reference "" Narrow; rebuilt "X" Narrow
- row 0, column 1, [Text]: reference "" Narrow; rebuilt "i" Narrow
- row 0, column 2, [Text]: reference "" Narrow; rebuilt "n" Narrow
- row 0, column 3, [Text]: reference "" Narrow; rebuilt "p" Narrow
- row 0, column 4, [Text]: reference "" Narrow; rebuilt "u" Narrow
- row 0, column 5, [Text]: reference "" Narrow; rebuilt "t" Narrow
- and 15 more cells

### split-pause / still-line-drawing, after more output and input: differs (2 cells): the character set in use is not exported

- row 0, column 2, [Text]: reference "─" Narrow; rebuilt "q" Narrow
- row 0, column 3, [Text]: reference "┐" Narrow; rebuilt "k" Narrow

### resize / alternate-resize, after more output and input: differs (36 cells): a capture returns the saved primary screen at the current width

- row 0, column 0, [Text]: reference "3" Narrow; rebuilt "P" Narrow
- row 0, column 1, [Text]: reference "4" Narrow; rebuilt "1" Narrow
- row 0, column 2, [Text]: reference "5" Narrow; rebuilt " " Narrow
- row 0, column 3, [Text]: reference "6" Narrow; rebuilt "0" Narrow
- row 0, column 4, [Text]: reference "" Narrow; rebuilt "1" Narrow
- row 0, column 5, [Text]: reference "" Narrow; rebuilt "2" Narrow
- and 30 more cells
