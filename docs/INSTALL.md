# Install tmux-workspaces

The native app needs no Rust, Zig, or Python installation. It uses **tmux 3.3 or
newer** for terminal sessions. Install tmux with your system package manager, for
example `brew install tmux` on macOS or `sudo apt install tmux` on Ubuntu.

## Install a package

Builds are architecture-specific: Apple silicon and Intel on macOS, and ARM64
and x86-64 on glibc Linux. Linux packages built by the workflow target Ubuntu
24.04 or newer, with a graphical desktop. Older distributions and musl-based
systems such as Alpine are not supported by those packages.

Once native packages are available on the [releases page](https://github.com/eandualem/tmux-workspaces-rs/releases),
install the latest build with:

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/tmux-workspaces-rs/main/install.sh | sh
```

The installer downloads the matching archive and checks its SHA-256 checksum
before extracting or running it. You can also download and inspect `install.sh`
first, then run it with `sh install.sh`. Checksums detect corrupt downloads;
they rely on the same GitHub release as the archive.

For a local build or a manually downloaded archive, keep the archive and its
`.sha256` file together and run:

```sh
sh install.sh --archive /path/to/tmux-workspaces-aarch64-apple-darwin.tar.gz
```

Use the filename for your platform. Building packages is described in
[Contributing](../CONTRIBUTING.md#build-installable-packages). The source tree's
initial development version is `0.0.0`; a package's `BUILD.json` records its exact
source revision, dirty state, target, compiler and native optimization settings.

## macOS

The installer places `tmux-workspaces.app` in `~/Applications`. Open that folder
in Finder and launch the app. It uses the normal Dock and application menu.
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
`PATH` if needed, or launch **tmux-workspaces** from the applications menu.

The desktop must provide system fonts, fontconfig, FreeType, XCB, xkbcommon,
xkbcommon-x11, and a usable Vulkan or EGL/OpenGL driver. Wayland sessions also
need the Wayland client library. **Open Library…** uses `xdg-desktop-portal` and
a portal backend for your desktop, normally installed by the distribution.
On Ubuntu 24.04, the basic runtime packages are:

```sh
sudo apt install tmux libfontconfig1 libfreetype6 libxcb1 libxkbcommon0 \
  libxkbcommon-x11-0 libwayland-client0 libvulkan1 libegl1 xdg-desktop-portal
```

Keep your desktop's portal backend and GPU driver installed. A headless server
needs a graphical session to use the app. Each Linux package includes
`ELF-REQUIREMENTS.txt` with the binary's actual library and symbol requirements.

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
