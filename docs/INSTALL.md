# Install Agent Interface

You do not need to install Rust or Zig yourself. The installer installs Agent
Interface; it does not install Agent Backbone (see
[Connect Agent Backbone](#connect-agent-backbone)). Terminal sessions require
**tmux 3.4 or newer**.

The executable, archives, installation and data paths use `agent-interface`.
The desktop app is named **Agent Interface**.

## Runtime prerequisites

Builds are architecture-specific: Apple silicon and Intel on macOS, and ARM64
and x86-64 on glibc Linux. Linux packages built by the workflow target Ubuntu
24.04 or newer, with a graphical desktop. Older distributions and musl-based
systems such as Alpine are not supported by those packages.

On Ubuntu under WSL, the installer installs missing runtime packages and registers
the Windows Start entry, asking for your Linux password through sudo when needed.
On other systems, install the prerequisites before running the installer, which
checks the packaged executable before finishing. On macOS, if you use Homebrew, run:

```sh
brew install tmux
```

On Linux, the desktop must provide system fonts, fontconfig, FreeType, XCB, xkbcommon,
xkbcommon-x11, and a usable Vulkan or EGL/OpenGL driver. HTML Documents need WebKitGTK 4.1. Wayland sessions also
need the Wayland client library and XWayland; the app uses X11 for embedded
Documents on both Wayland desktops and WSLg. **Open Library…** uses `xdg-desktop-portal` and
a portal backend for your desktop, normally installed by the distribution.
On Ubuntu 24.04, the basic runtime packages are:

```sh
sudo apt install curl tmux fonts-dejavu-core libfontconfig1 libfreetype6 libxcb1 libxkbcommon0 \
  libxkbcommon-x11-0 libwayland-client0 libvulkan1 libegl1 libwebkit2gtk-4.1-0 xwayland xdg-desktop-portal
```

Keep your desktop's portal backend and GPU driver installed. A headless server
needs a graphical session to use the app. Each Linux package includes
`ELF-REQUIREMENTS.txt` with the binary's actual library and symbol requirements.

Use the [latest stable tmux release](https://github.com/tmux/tmux/releases/latest).
Run `tmux -V` and confirm it reports at least 3.4, the minimum required for
capture buffers that do not interfere with ordinary paste. An already-running
server must also be 3.4 or newer; upgrading the executable alone does not upgrade
existing servers. Finish or save work in old sessions before restarting them.
The app rejects an older server without terminating its sessions.

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

Downloads show a progress bar. Package installation steps print elapsed-time
updates every five seconds while running; these updates indicate that the step
is still active, not a percentage of work completed.

For a local build or a manually downloaded archive, download the archive and its
matching `.tar.gz.sha256` file into the same folder. Keep the archive compressed.
Choose the filename for your computer:

- Apple silicon Mac: `agent-interface-aarch64-apple-darwin.tar.gz`
- Intel Mac: `agent-interface-x86_64-apple-darwin.tar.gz`
- ARM64 Linux: `agent-interface-aarch64-unknown-linux-gnu.tar.gz`
- x86-64 Linux: `agent-interface-x86_64-unknown-linux-gnu.tar.gz`

Keep each archive and its matching `.tar.gz.sha256` file under their original
names. Download the installer, inspect it, then point it at your archive. This
example uses Apple silicon; substitute the filename you downloaded for your
computer.

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh -o install.sh
sh install.sh --archive "$HOME/Downloads/agent-interface-aarch64-apple-darwin.tar.gz"
```

Building packages is described in
[Contributing](../CONTRIBUTING.md#build-installable-packages). A package's
`BUILD.json` records its version, exact source revision, dirty state, target,
compiler and native optimization settings.

## macOS

The installer places `Agent Interface.app` in `~/Applications`. Open
`~/Applications` in Finder and launch `Agent Interface.app`, or run:

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

### Linux document sandbox

Documents use WebKitGTK's process sandbox. If Ubuntu's namespace restrictions
prevent Bubblewrap from starting, the installer offers sudo setup using the
Ubuntu `apparmor-profiles` package's `bwrap-userns-restrict` policy. This policy
attaches to the root-owned `/usr/bin/bwrap` helper and restricts capabilities in
its children. The installer preserves existing policy files and loaded profiles;
it does not grant permissions to user-writable app paths or change global sysctls.
If setup is declined or unavailable, installation continues with native views;
Documents explain that sandbox support needs configuration by an administrator.

Containers must allow nested user, mount, PID and network namespaces for WebKitGTK.
The app checks sandbox startup before opening Documents and reports an unavailable
sandbox without disabling native terminals. Do not set
`WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS`; Documents refuse that override.

## Windows 11 with WSL

Use the Linux package inside Ubuntu 24.04 or newer on **WSL 2 with WSLg**.
There is no native Windows executable. The installer runs inside your Linux
distribution and registers the app in Windows Start. Agent Backbone is optional
for terminals and saved workspaces.

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
Run the installer in its Linux terminal, not PowerShell:

```sh
curl -fsSL https://raw.githubusercontent.com/eandualem/agent-interface/main/install.sh | sh
```

The installer installs missing runtime packages, downloads the prebuilt app,
and adds **Agent Interface** under your distribution's name in Windows Start. Enter your Linux password if sudo asks for it. No compiler
or source checkout is needed. Installation speed and GPU rendering under WSL
have not been measured.

Open **Agent Interface** from Windows Start to begin setup. Workspaces in the
app's own data folder (see [Update or remove](#update-or-remove)) are preserved;
setup appears until you finish it. You can also launch it inside
WSL with `"$HOME/.local/bin/agent-interface"`.
Use **Interface size** at the bottom of the welcome window if the text is too
small. Run agent-tool installation commands inside WSL as well. After installing
a tool, choose **Check again** or **Check tools again** in setup; you do not need
to restart the app.

After setup, add a tab with Ctrl+Shift+T, choose **Open terminal**, and run
`pwd`. Quit and reopen the app to check that the saved workspace and terminal
reconnect. The terminals run inside that Ubuntu distribution. On a
high-resolution display WSL can draw the window too small; change
**Settings → Appearance → Interface size** to suit it. If no window
appears, check
[Microsoft's WSLg display troubleshooting](https://github.com/microsoft/wslg/wiki/Diagnosing-%22cannot-open-display%22-type-issues-with-WSLg)
and the app's [diagnostic logging](GUIDE.md#find-work-and-change-appearance).

## Connect Agent Backbone

The installer does not install Agent Backbone, and running it again does not
add it: the Agent Backbone service is not distributed yet
([#268](https://github.com/eandualem/agent-interface/issues/268)). With the
service installed, open Agent Interface to continue onboarding. Onboarding
creates your first profile, and offers to install Git, tmux or an agent tool
when one is missing; each agent tool signs in inside its own terminal. Without
Agent Backbone, Agent Interface works as a native workspace for tmux: first run
stops at **Agents can't start yet**, and **Continue with terminals** opens the
workspace. Running the installer again, which **Details** may suggest, does
not add the service.

The app finds the profiles Agent Backbone serves through its `backbone` command
and connects to each over the private Unix socket the service reports. The
socket must be owned by your user with permissions 0600, in a directory owned
by your user with permissions 0700. The app does not send credentials over TCP.

## Update or remove

Quit the app and run the same install command again. The installer stages and
checks the replacement before updating its own installation. Layouts and
settings in the app's own folders, `${XDG_DATA_HOME:-~/.local/share}/agent-interface`
and `${XDG_CONFIG_HOME:-~/.config}/agent-interface` on macOS and Linux alike, are
preserved. It refuses to overwrite an installation it does not recognize; move
that installation yourself if you want to replace it.

You can open a library in another folder with **Open Library…** (Command-O, or
Ctrl+Shift+O on Linux), **Open another** in Settings, `--data-dir` or
`AGENT_INTERFACE_DATA_DIR`. These change only the library folder; settings stay
in the configuration folder above.

The installer retains one archive in a hidden download cache so interrupted
installs and repeated attempts can reuse it. Include that cache when removing
the installation.

To remove the macOS app, quit it and move `~/Applications/Agent Interface.app` to
the Trash, then remove its download cache:

```sh
rm -rf "$HOME/Applications/.agent-interface-download"
```

On Linux, quit the app and remove these installation files:

```sh
rm -rf "$HOME/.local/lib/agent-interface" "$HOME/.local/lib/.agent-interface-download"
rm -f "$HOME/.local/bin/agent-interface"
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/applications/io.github.eandualem.agent-interface.desktop"
```

The distribution Bubblewrap policy is shared system sandbox support, like the
runtime libraries. Uninstalling this app does not remove it or change other apps'
sandbox configuration.

These commands leave your data and terminal sessions intact. See the
[user guide](GUIDE.md) for library and settings locations.

On WSL, also remove the Windows Start registration only if its installer marker
and recorded home directory identify your Linux user:

```sh
wsl_launcher=/usr/share/applications/io.github.eandualem.agent-interface.desktop
if [ -f "$wsl_launcher" ] && [ ! -L "$wsl_launcher" ] &&
    grep -qx 'X-AgentInterface-Installer=1' "$wsl_launcher" &&
    grep -Fqx "X-AgentInterface-Home=$HOME" "$wsl_launcher"; then
    sudo rm -f -- "$wsl_launcher"
else
    printf '%s\n' 'No installer-owned Windows launcher for this user; leaving it unchanged.'
fi
```
