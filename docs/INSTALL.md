# Install Agent Interface

Native packages need no Rust, Zig, or Python installation. It uses **tmux 3.3 or
newer** for terminal sessions.

The executable, archives and new installation/data paths use `agent-interface`.
The desktop app is named **Agent Interface**. Existing libraries and preferences
from earlier installations are still read without moving their data.

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
sudo apt install curl tmux fonts-dejavu-core libfontconfig1 libfreetype6 libxcb1 libxkbcommon0 \
  libxkbcommon-x11-0 libwayland-client0 libvulkan1 libegl1 xdg-desktop-portal
```

Keep your desktop's portal backend and GPU driver installed. A headless server
needs a graphical session to use the app. Each Linux package includes
`ELF-REQUIREMENTS.txt` with the binary's actual library and symbol requirements.

Run `tmux -V` and confirm it reports 3.3 or newer.

## Install a package

Check the [releases page](https://github.com/eandualem/agent-interface/releases)
for available packages. The command below needs a published release with an
archive for your platform; an empty release list or a missing archive means it
is not available yet. The installer downloads the compiled app and does not
build it on your computer.

With the runtime prerequisites installed, install the latest release with:

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

- Apple silicon Mac: `agent-interface-aarch64-apple-darwin.tar.gz`
- Intel Mac: `agent-interface-x86_64-apple-darwin.tar.gz`
- ARM64 Linux: `agent-interface-aarch64-unknown-linux-gnu.tar.gz`
- x86-64 Linux: `agent-interface-x86_64-unknown-linux-gnu.tar.gz`

Release 0.1.0 uses `tmux-workspaces-` instead of `agent-interface-` for
these same targets. The installer accepts both naming generations. Keep each
archive and its matching `.tar.gz.sha256` file under their original names.

Download the installer, inspect it, then point it at your archive. This example
uses Apple silicon; substitute the filename you downloaded for your computer
and release.

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh -o install.sh
sh install.sh --archive "$HOME/Downloads/agent-interface-aarch64-apple-darwin.tar.gz"
```

For the 0.1.0 Apple silicon archive, use this installation command instead:

```sh
sh install.sh --archive "$HOME/Downloads/tmux-workspaces-aarch64-apple-darwin.tar.gz"
```

Building packages is described in
[Contributing](../CONTRIBUTING.md#build-installable-packages). A package's
`BUILD.json` records its version, exact source revision, dirty state, target,
compiler and native optimization settings.

## macOS

The installer places `Agent Interface.app` in `~/Applications`, replacing an
older `tmux-workspaces.app` installed by this installer. Open `~/Applications`
in Finder and launch `Agent Interface.app`, or run:

```sh
open "$HOME/Applications/Agent Interface.app"
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

The installer places the app in `~/.local/lib/agent-interface`, adds a command in
`~/.local/bin`, and creates a desktop launcher under
`${XDG_DATA_HOME:-~/.local/share}/applications`. Add `~/.local/bin` to your shell's
`PATH` if needed, or launch **Agent Interface** from the applications menu.

You can launch directly without changing `PATH`:

```sh
"$HOME/.local/bin/agent-interface"
```

Updates from the earlier `tmux-workspaces` installation preserve saved libraries
and settings. The installer replaces its owned legacy app and launchers; the new
command is `agent-interface`. Existing data directories remain readable in place.

## Windows 11 with WSL (experimental)

Use the Linux package inside Ubuntu 24.04 or newer on **WSL 2 with WSLg**.
There is no native Windows executable. This app's WSL compatibility has not
been verified; the steps below are for testing it once a Linux release is
available. Agent Backbone is optional for terminals and saved workspaces.

If WSL or a suitable Ubuntu distribution is missing, run this in an
**administrator PowerShell**, restart Windows if prompted, and complete Ubuntu's
username/password setup:

```powershell
wsl --install -d Ubuntu-24.04
```

This installs Ubuntu 24.04 as a separate distribution; it does not upgrade an
older Ubuntu installation. See [Microsoft's WSL commands](https://learn.microsoft.com/en-us/windows/wsl/basic-commands)
for available distributions and WSL 1 to WSL 2 conversion.

In **PowerShell**, check your existing WSL installation:

```powershell
wsl --version
wsl --list --verbose
```

The Ubuntu distribution must show version `2`, and `wsl --version` should list
WSLg. If WSL needs updating, run `wsl --update` from an administrator PowerShell.
Follow [Microsoft's WSL GUI setup](https://learn.microsoft.com/en-us/windows/wsl/tutorials/gui-apps)
for GPU drivers and restarting WSL after an update. Save your work first:
`wsl --shutdown` stops every running WSL distribution, including its tmux
sessions and processes. An ordinary app install does not require that shutdown.

Open your **Ubuntu 24.04 or newer** distribution from the Windows Start menu.
Run the remaining commands in its Linux terminal. Check the Ubuntu version:

```sh
cat /etc/os-release
```

On Ubuntu 24.04 or newer, install the runtime prerequisites once:

```sh
sudo apt update
sudo apt install -y curl tmux fonts-dejavu-core libfontconfig1 libfreetype6 \
  libxcb1 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libvulkan1 \
  libegl1 libgl1-mesa-dri mesa-vulkan-drivers xdg-desktop-portal \
  xdg-desktop-portal-gtk
```

Then install and launch the app:

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh | sh
"$HOME/.local/bin/agent-interface"
```

This downloads a prebuilt Linux app; no compiler or source checkout is needed.
The prerequisite downloads and installation take additional time on a fresh
Ubuntu installation. Installation speed and GPU rendering under WSL have not
been measured.

For a first test, add a tab with Ctrl+Shift+T, choose **Open terminal**, and run
`pwd`. Quit and reopen the app to check that the saved workspace and terminal
reconnect. The terminals run inside that Ubuntu distribution. If no window
appears, check
[Microsoft's WSLg display troubleshooting](https://github.com/microsoft/wslg/wiki/Diagnosing-%22cannot-open-display%22-type-issues-with-WSLg)
and the app's [diagnostic logging](GUIDE.md#find-work-and-change-appearance).

## Connect Agent Backbone

The agents list and agent views read from a running
[Agent Backbone](https://github.com/eandualem/agent-backbone) on the same
computer. The app sends Backbone's API key only over a private Unix socket,
which Backbone added after its 0.3.0 release. Until a newer release is
published, install Backbone from its repository. You need
[uv](https://docs.astral.sh/uv/), Git, Python 3.11 or newer, and an agent CLI
that is installed and signed in. This command also replaces an existing
installation made with `uv tool install`:

```sh
uv tool install git+https://github.com/eandualem/agent-backbone
```

Then set it up and start its service with Backbone's
[quick start](https://github.com/eandualem/agent-backbone#quick-start),
skipping its install command.

Enable the socket in Backbone's data directory, then restart its service so it
binds the socket:

```sh
mkdir -p "$HOME/.local/share/agent-backbone/run"
chmod 700 "$HOME/.local/share/agent-backbone/run"
backbone config set backbone.socket_path "$HOME/.local/share/agent-backbone/run/backbone.sock"
backbone service restart
```

If Agent Interface was already open before you first set up Backbone, quit and
reopen it so it finds Backbone.

The app reads the saved `backbone.socket_path` setting. To use another socket,
set `BACKBONE_SOCKET_PATH` in the app's launch environment. The path must be
absolute. The socket must be owned by your user with permissions 0600, in a
directory owned by your user with permissions 0700. The app does not send
credentials over TCP.

## Update or remove

Quit the app and run the same install command again. The installer stages and
checks the replacement before updating its own installation. Existing layouts,
settings, and tmux sessions are preserved. It refuses to overwrite an installation
it does not recognize; move that installation yourself if you want to replace it.

To remove the macOS app, quit it and move `~/Applications/Agent Interface.app` to
the Trash. On Linux, quit it and remove these installation files:

```sh
rm -rf "$HOME/.local/lib/agent-interface"
rm -f "$HOME/.local/bin/agent-interface"
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.eandualem.agent-interface.desktop"
```

These commands leave your data and terminal sessions intact. See the
[user guide](GUIDE.md) for library and settings locations.
