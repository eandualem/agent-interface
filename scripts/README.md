# Repository scripts

These scripts automate work in this repository: preparing native sources,
packaging, keeping build output small, the Linux checks and the installer's
tests. They are written in Python because Rust has no scripts.

This folder holds the only Python in the repository. None of it ships in the app
or its packages, and neither the app nor `install.sh` runs Python.

| Script | What it does |
|---|---|
| `prepare-native.py` | Fetches the pinned Ghostty source, applies `native/ghostty.patch`, checks the result and prints its folder under `.native/`. Run it after cloning or changing native inputs. |
| `sweep_target.py` | Runs the checks every commit needs (`cargo fmt --check`, `cargo clippy`, a build and the tests) and deletes every build artifact the cycle did not use, so `target/` keeps only the latest build. |
| `package.py` | Builds the optimized app for this computer and packs it with its notices into an installable archive and checksum in `dist/`. |
| `test-linux-desktop.py` | Opens the real Linux window in a disposable Xvfb desktop and checks it, with private library, home and tmux state. CI runs it. |
| `test-linux-onboarding.py` | Clicks through first run in a private Linux desktop against a stand-in Agent Backbone. `test-linux-desktop.py` runs it. |
| `test-linux-documents.py` | Renders a retained Document in a disposable Xvfb desktop with WebKit's sandbox on. CI runs it. |
| `tests/test_install.py` | Tests `install.sh` with inert archives in private home folders. |
| `tests/test_sweep_target.py` | Tests which artifacts `sweep_target.py` keeps. |
| `tests/fixtures/fake_backbone.py` | A stand-in for the Agent Backbone service, used by the Linux checks. |

Run the script tests with:

```sh
python3 -B -m unittest discover -s scripts/tests -v
```

The commands for building, testing and packaging are in
[Contributing](../CONTRIBUTING.md).
