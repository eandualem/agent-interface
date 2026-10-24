# Install Agent Interface

Native packages need no Rust, Zig, or Python installation. It uses **tmux 3.3 or
newer** for terminal sessions.

The executable, archive names, application bundle path and data directories retain
`tmux-workspaces` for compatibility with existing installations and libraries.
The product and repository are named Agent Interface.

## Runtime prerequisites

Builds are architecture-specific: Apple silicon and Intel on macOS, and ARM64
and x86-64 on glibc Linux. Linux packages built by the workflow target Ubuntu
24.04 or newer, with a graphical desktop. Older distributions and musl-based
systems such as Alpine are not supported by those packages.

Install the prerequisites before running the installer, which checks the
packaged executable before finishing. On macOS, if you use Homebrew, run:

```sh
brew install tmux
```

On Linux, the desktop must provide system fonts, fontconfig, FreeType, XCB, xkbcommon,
xkbcommon-x11, and a usable Vulkan or EGL/OpenGL driver. Wayland sessions also
need the Wayland client library. **Open Library…** uses `xdg-desktop-portal` and
a portal backend for your desktop, normally installed by the distribution.
On Ubuntu 24.04, the basic runtime packages are:

```sh
sudo apt install curl tmux libfontconfig1 libfreetype6 libxcb1 libxkbcommon0 \
  libxkbcommon-x11-0 libwayland-client0 libvulkan1 libegl1 xdg-desktop-portal
```

Keep your desktop's portal backend and GPU driver installed. A headless server
needs a graphical session to use the app. Each Linux package includes
`ELF-REQUIREMENTS.txt` with the binary's actual library and symbol requirements.

Run `tmux -V` and confirm it reports 3.3 or newer.

## Install a package

Native packages have not been published yet. To run the app now, follow the
[source build instructions](../CONTRIBUTING.md#set-up).

Once native packages are available on the [releases page](https://github.com/eandualem/agent-interface/releases),
install the latest build with:

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh | sh
```

The installer downloads the matching archive and checks its SHA-256 checksum
before extracting or running it. You can also download and inspect `install.sh`
first, then run it with `sh install.sh`. Checksums detect corrupt downloads;
they rely on the same GitHub release as the archive.

For a local build or a manually downloaded archive, download the archive and its
matching `.tar.gz.sha256` file into the same folder. Keep the archive compressed.
Choose the filename for your computer:

- Apple silicon Mac: `tmux-workspaces-aarch64-apple-darwin.tar.gz`
- Intel Mac: `tmux-workspaces-x86_64-apple-darwin.tar.gz`
- ARM64 Linux: `tmux-workspaces-aarch64-unknown-linux-gnu.tar.gz`
- x86-64 Linux: `tmux-workspaces-x86_64-unknown-linux-gnu.tar.gz`

Download the installer, inspect it, then point it at your archive. This example
uses Apple silicon; substitute the matching filename for your computer.

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh -o install.sh
sh install.sh --archive "$HOME/Downloads/tmux-workspaces-aarch64-apple-darwin.tar.gz"
```

Building packages is described in
[Contributing](../CONTRIBUTING.md#build-installable-packages). A package's
`BUILD.json` records its version, exact source revision, dirty state, target,
compiler and native optimization settings.

## macOS

The installer places `tmux-workspaces.app` in `~/Applications`. Open
`~/Applications` in Finder and launch `tmux-workspaces.app`, or run:

```sh
open "$HOME/Applications/tmux-workspaces.app"
```

The app uses the normal Dock and application menu.
Homebrew's standard Apple silicon and Intel tmux locations are recognized even
when Finder has a different `PATH` from your terminal.

The app is ad-hoc signed, without an Apple Developer certificate or notarization.
If macOS blocks the first launch, try opening the app, then use **System Settings
→ Privacy & Security → Open Anyway** for this app. See
[Apple's instructions](https://support.apple.com/en-us/102445). The installer
does not remove quarantine attributes or disable Gatekeeper.

## Linux

The installer places the app in `~/.local/lib/tmux-workspaces`, adds a command in
`~/.local/bin`, and creates a desktop launcher under
`${XDG_DATA_HOME:-~/.local/share}/applications`. Add `~/.local/bin` to your shell's
`PATH` if needed, or launch **Agent Interface** from the applications menu.

## Update or remove

Quit the app and run the same install command again. The installer stages and
checks the replacement before updating its own installation. Existing layouts,
settings, and tmux sessions are preserved. It refuses to overwrite an installation
it does not recognize; move that installation yourself if you want to replace it.

To remove the macOS app, quit it and move `~/Applications/tmux-workspaces.app` to
the Trash. On Linux, quit it and remove these installation files:

```sh
rm -rf "$HOME/.local/lib/tmux-workspaces"
rm -f "$HOME/.local/bin/tmux-workspaces"
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.eandualem.tmux-workspaces.desktop"
```

These commands leave your data and terminal sessions intact. See the
[user guide](GUIDE.md) for library and settings locations.
