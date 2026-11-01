# Security

Agent Interface is a desktop client for your tmux sessions and
[Agent Backbone](https://github.com/eandualem/agent-backbone). It runs under
your account. This page describes its responsibilities, reporting process,
and known limits.

## Report a vulnerability

Report suspected security defects privately through the repository's
**Security → Report a vulnerability** form. Use a public issue for ordinary
bugs that do not expose sensitive information.

Include the app version (**About Agent Interface**, or `BUILD.json`), operating
system, tmux version, relevant configuration, and a minimal reproduction.
Describe what you expected, what happened, and the concrete impact on the
product: for example, incorrect terminal output, unintended input routing,
access to another account's data, or an added capability exceeding its intended
access. Use a supported setup and remove private data from examples. If you
cannot safely reproduce a suspected defect, report the evidence and uncertainty;
a complete exploit is not required.

Agent actions that their CLI harness already permits are not by themselves an
Interface defect. Reports of defects in Interface's own handling or capabilities
are still welcome, including when an agent or terminal output triggers them.
Reports are handled in their private advisory thread. Confirmed security fixes
are included in a subsequent release, with credit unless you request otherwise.
Only the latest release receives security fixes.

## Responsibilities and boundaries

**Your agents are your tools.** Their execution permissions, approval prompts
and sandboxing belong to the CLI harness that runs them, such as Codex, Claude
Code or OpenCode. Configure those controls in the harness. Interface does not
provide a general containment boundary for an agent running under your account.
Other programs with the same account permissions can access your files and tmux
sessions directly.

**Interface owns the behavior it adds.** It must render terminal data faithfully,
route input to the selected pane, keep terminal data separate from control
messages, and apply the intended identity and access checks to its Backbone and
document capabilities. It opens no network listener and runs no daemon of its own.

| Capability | Current behavior |
| --- | --- |
| Terminal output | Printed bytes are parsed as terminal data. Output does not automatically open links, run commands in the app, or access the clipboard. An allocated operating system command (OSC) sequence is discarded at 1 MiB, matching tmux's default input limit, so an unfinished sequence cannot keep growing the parser's memory. This is terminal robustness, not an agent execution policy. |
| Paste and dropped files | Text goes to the selected pane, with bracketed paste when enabled. Multiline paste is refused when that mode cannot be verified; a single paste is limited to 4 MiB. These safeguards help prevent unintended input. |
| tmux | Commands target numeric pane and window IDs, with quoted shell arguments. Ordinary shells use a tmux socket in a private directory owned by your account. |
| Agent Backbone | Credential-bearing requests use a Unix socket owned by your account, with a 0600 socket in a 0700 directory. Both ends check peer identity. There is no network fallback for these requests. This distinguishes accounts, not programs running as the same account. |
| HTML documents | The document view has no network access, navigation, downloads, new windows or file chooser. WebRTC is disabled. On Linux, clipboard access is disabled and WebKit's process sandbox must be active. The view refuses to open if these settings cannot be applied. These restrictions belong to the embedded document capability, not to your terminal sessions. |
| Links | Clicking an app-provided report or document link can open an `http` or `https` URL in your browser. |
| Setup | First-run setup displays official installation commands for agent tools; it does not execute those installation commands. |

## Optional diagnostics and functional data

In **Settings → General → Diagnostic logging**, **Off** disables diagnostic
records. **Warnings** is the quiet default; **Debug** explicitly enables lifecycle
detail, and **Trace** adds operation timings. The choice is saved and applies
without restarting. An Off launch creates no diagnostic log files. Switching Off
stops new records; records already queued may finish writing, and existing files
remain until you delete them.

Diagnostics are local, bounded to four 4 MiB files, and never uploaded
automatically. Instrumentation is designed around event metadata rather than
terminal contents, typed commands or request bodies. Detailed records can still
reveal identifiers, local paths, error context or workflow information. Choose
Debug or Trace only when that local retention is acceptable, inspect files before
sharing, and delete them when no longer needed. No AI or regular-expression
filter can guarantee that arbitrary diagnostic text is free of secrets; the app
does not promise universal secret scrubbing. See the
[logging guide](docs/GUIDE.md#find-work-and-change-appearance) for file locations.

Off controls optional diagnostics, not the data needed to perform the work:
saved layouts and settings, setup requests and resumable progress, terminal
history held by tmux, and data retained by Backbone or your agent harness remain
subject to their own lifecycle. User-visible operation errors also remain
available when diagnostics are off.

## Distribution limits

The installer checks downloads against their published SHA-256 checksums. Both
come from the same GitHub release, so checksums detect corruption but do not by
themselves establish who built an archive. Build provenance verification is
described below. The macOS app is ad-hoc signed and is not notarized by Apple.

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
from any other commit.

Archives from releases before attestations were added (0.2.6 and earlier)
have checksums only.

## How the app is tested and reviewed

Every change goes through the same checks before it reaches a release:

- **Continuous integration** on macOS and Linux runs formatting, strict lint
  checks with warnings treated as errors, and the full test suite. The suite
  includes regression tests for protections above, such as refusing multiline
  paste without bracketed paste and the 1 MiB limit on stored OSC sequences.
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
  installer. It confirmed that an unfinished OSC sequence the terminal stores
  could hold unbounded memory, now fixed with the 1 MiB limit. It also confirmed a
  second problem, still open below.

Not done yet: there is no fuzz testing and no third-party audit.

## Open security work

- **Captured text that imitates tmux's reply marker.** When the app reads a
  pane's existing content, tmux sends that text unchanged and marks the end of
  its reply with a line such as `%end`. A program that prints a line matching
  the exact marker for that request ends the reply early. The app then reads
  the rest of the captured text as messages from tmux. That text could draw
  forged output in another pane of the same window, change the displayed
  layout, or disconnect the window. It cannot type into panes, run commands of
  its choosing or read data. Matching the marker means guessing the exact
  second and tmux's server-wide command number for that request. A fix that
  keeps captured text out of tmux's reply stream is being worked on in
  [#88](https://github.com/eandualem/agent-interface/issues/88).

This page is updated when a review finishes or a security fix is released.
