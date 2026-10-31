# Security

Agent Interface shows your tmux sessions, types into them and reads your
[Agent Backbone](https://github.com/eandualem/agent-backbone) service. This
page explains what the app protects, what it does not, how it is tested, and
which security work is still open.

## Report a vulnerability

Please report security problems privately. Open the repository's **Security**
tab and choose **Report a vulnerability**. Do not open a public issue.

Include the version (**About Agent Interface**, or `BUILD.json` in the
package), your platform, and the steps or input that show the problem. We
aim to reply within a week. A confirmed problem is fixed in a new release,
and the advisory credits you unless you ask us not to.

Only the latest release receives security fixes.

## Security model at a glance

**The app runs as you, on your computer.** It opens no network port and runs
no background service of its own. Any program running under your user account
can already read your files and use your tmux sessions, so the app does not
try to defend against your own account. Agents keep the permissions and
sandbox of the tool that runs them (Claude Code, Codex, OpenCode and so on);
Agent Interface does not widen or narrow them.

What the app does protect:

| Boundary | Protection |
| --- | --- |
| Terminal output | What a program prints is treated as untrusted. It cannot read or set your clipboard, open links or run commands in the app. An unfinished control sequence is discarded after 1 MiB, the same limit tmux uses, so it cannot make the app hold ever more memory. |
| Paste and dropped files | Pasted text is sent only to the pane you chose, with bracketed paste when the shell enabled it. If the app cannot tell whether bracketed paste is on, multiline paste is refused. A single paste is limited to 4 MiB. |
| tmux | Commands address panes and windows by their numeric IDs, and shell arguments are quoted. Ordinary shells run on a tmux server whose socket sits in a private directory owned by you. |
| Agent Backbone | Requests that carry your Backbone credential go only over a Unix socket owned by you (mode 0600, in a 0700 directory). Before sending anything, the app checks that the process on the other end runs as your user, and Backbone checks the same of the app. There is no fallback to a network connection for these requests. This keeps other accounts out; it does not separate programs running as you. |
| HTML documents | Documents open in a system web view with no network access, no navigation, no downloads, no new windows and no file chooser. WebRTC is disabled. On Linux, scripts cannot use the clipboard and WebKit's process sandbox must be active. If any of these settings cannot be applied, the document does not open. |
| Links | Only links in the app's own report and document views open, and only `http` and `https` addresses, when you click them. |
| Logs | Logs record timings and outcomes, never terminal content, typed input, request bodies or credentials. |
| Setup | First-run setup shows the official install commands for agent tools. It never runs them for you. |

## What it does not protect against

- **Other programs running as you.** They can read the app's data, your
  Backbone credential and your tmux sessions directly.
- **What your agents do.** An agent can do whatever its own tool allows.
  Review each tool's permission and sandbox settings.
- **A compromised release.** The installer checks each download against the
  SHA-256 checksum published with it, which catches corrupt downloads. Both
  files come from the same GitHub release, so checksums alone do not prove who
  built them. See [Verify a download](#verify-a-download).
- **macOS Gatekeeper review.** The macOS app is ad-hoc signed and not
  notarized by Apple.

## Verify a download

Release archives built by GitHub Actions carry a signed build provenance
attestation. It records which workflow, commit and repository produced the
archive. With the [GitHub CLI](https://cli.github.com) installed, check an
archive against the commit its release tag points to (replace `vX.Y.Z` with
the release you downloaded):

```sh
commit=$(gh api repos/eandualem/agent-interface/commits/vX.Y.Z --jq .sha) &&
gh attestation verify agent-interface-aarch64-apple-darwin.tar.gz \
  --repo eandualem/agent-interface \
  --signer-workflow eandualem/agent-interface/.github/workflows/packages.yml \
  --source-digest "${commit:?release commit not found}"
```

Checking the commit matters: it rejects an archive the same workflow built
from any other branch or commit.

Archives from releases before attestations were added (0.2.6 and earlier)
have checksums only.

## How the app is tested and reviewed

Every change goes through the same checks before it reaches a release:

- **Continuous integration** on macOS and Linux runs formatting, strict lint
  checks with warnings treated as errors, and the full test suite. The suite
  includes regression tests for protections above, such as refusing multiline
  paste without bracketed paste and the limit on unfinished control sequences.
- **Dependency audit:** [`cargo deny`](https://github.com/EmbarkStudios/cargo-deny)
  checks every Rust dependency against the RustSec advisory database, and
  checks licences and download sources. Any known vulnerability fails the
  build. The only accepted advisories are notices that a few indirect
  dependencies are no longer maintained, with no known vulnerability; each is
  listed with its reason in [`deny.toml`](deny.toml). One GLib bug
  (RUSTSEC-2024-0429) is fixed by a local patch, described in the
  [architecture guide](docs/ARCHITECTURE.md#dependencies-and-licences).
- **Pinned build inputs:** Rust dependencies are locked, fast-moving ones to
  exact versions. The terminal engine is pinned to a reviewed commit with a
  checksummed patch. Every GitHub Action is pinned to a full commit SHA, and
  workflows have read-only access unless a step needs more.
- **Independent review:** each change is reviewed by a second AI model in a
  separate, read-only context, and pull requests also get an automated
  CodeRabbit review. Valid findings are fixed before merging.

Security reviews so far:

- **October 2026, reported findings:** 14 findings from an automated security
  scan were checked and fixed in
  [#116](https://github.com/eandualem/agent-interface/pull/116). That work
  added the private, identity-checked Backbone socket described above.
- **October 2026, trust boundary review**
  ([#88](https://github.com/eandualem/agent-interface/issues/88)): a source
  review of tmux command construction, terminal output, clipboard and links,
  saved layouts and file paths, Backbone data, and installation. It found no
  problem in command construction, quoting, storage, credentials or the
  installer. It confirmed that an unfinished control sequence could hold
  unbounded memory, now fixed with the 1 MiB limit. One candidate remains open
  below.

Not done yet: there is no fuzz testing and no third-party audit.

## Open security work

- **Captured text that imitates tmux's reply marker.** When the app reads a
  pane's existing content, tmux marks the end of its reply with a line such as
  `%end`. A program that prints a line matching the exact marker for that
  request could make the app treat the capture as failed and show an error
  instead of the pane. It cannot run commands or read data. This candidate is
  being checked in
  [#88](https://github.com/eandualem/agent-interface/issues/88).

This page is updated when a review finishes or a security fix is released.
