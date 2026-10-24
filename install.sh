#!/bin/sh
# Install the native desktop app for the current user. No sudo or build tools.
set -eu

# Wait for the complete script before changing anything when piped from curl.
main() {

fail() { printf 'tmux-workspaces: %s\n' "$*" >&2; exit 1; }
usage() {
    printf '%s\n' 'Usage: sh install.sh [--archive PATH]' \
        'Installs the latest published build, or a local archive with its .sha256 file.' \
        'Quit the app before updating. Layouts and running tmux sessions are preserved.'
}
archive=
case $# in
    0) ;;
    1) case $1 in --help|-h) usage; exit 0;; *) usage; exit 1;; esac ;;
    2) [ "$1" = --archive ] || { usage; exit 1; }; archive=$2 ;;
    *) usage; exit 1 ;;
esac

case $(uname -s) in
    Darwin) platform=apple-darwin ;;
    Linux) platform=unknown-linux-gnu ;;
    *) fail 'Only macOS and Linux are supported.' ;;
esac
case $(uname -m) in
    arm64|aarch64) arch=aarch64 ;;
    x86_64|amd64) arch=x86_64 ;;
    *) fail 'No build is available for this CPU architecture.' ;;
esac
target=$arch-$platform
case ${HOME:-} in /*) ;; *) fail 'HOME must be an absolute path.';; esac
# Desktop files cannot represent paths with line breaks or tabs.
case $HOME${XDG_DATA_HOME:-} in
    *'
'*) fail 'Installation paths must not contain line breaks.' ;;
esac
if printf '%s' "$HOME${XDG_DATA_HOME:-}" | LC_ALL=C grep -q '[[:cntrl:]]'; then
    fail 'Installation paths must not contain control characters.'
fi
if [ "$platform" = apple-darwin ]; then
    parent=$HOME/Applications
    destination=$parent/tmux-workspaces.app
    payload=tmux-workspaces.app
    executable=Contents/MacOS/tmux-workspaces
    marker=Contents/Resources/INSTALLER
else
    getconf GNU_LIBC_VERSION >/dev/null 2>&1 || fail 'This build requires glibc Linux (not musl).'
    parent=$HOME/.local/lib
    destination=$parent/tmux-workspaces
    payload=tmux-workspaces
    executable=bin/tmux-workspaces
    marker=INSTALLER
    data=${XDG_DATA_HOME:-$HOME/.local/share}
    case $data in /*) ;; *) fail 'XDG_DATA_HOME must be an absolute path.';; esac
    launcher=$data/applications/io.github.eandualem.tmux-workspaces.desktop
    link=$HOME/.local/bin/tmux-workspaces
    if [ -e "$launcher" ] || [ -L "$launcher" ]; then
        [ -f "$launcher" ] && grep -qx 'X-TmuxWorkspaces-Installer=1' "$launcher" || fail "Refusing to replace $launcher."
    fi
    if [ -e "$link" ] || [ -L "$link" ]; then
        [ -L "$link" ] && [ "$(readlink "$link")" = "$destination/$executable" ] || fail "Refusing to replace $link."
    fi
fi
if [ -e "$destination" ] || [ -L "$destination" ]; then
    [ ! -L "$destination" ] && [ -f "$destination/$marker" ] &&
        [ "$(cat "$destination/$marker")" = io.github.eandualem.tmux-workspaces ] ||
        fail "Refusing to replace an installation not owned by this installer: $destination"
fi
mkdir -p "$parent"
lock=$parent/.tmux-workspaces-install.lock
mkdir "$lock" 2>/dev/null || fail "Another install is active. If it was interrupted, remove $lock and retry."
staging=
backup=
launcher_stage=
launcher_backup=
installed=false
new_link=false
new_launcher=false
cleanup() {
    status=$?
    trap - EXIT HUP INT TERM
    set +e
    rollback_ok=true
    if [ "$status" -ne 0 ]; then
        if [ "$installed" = true ]; then rm -rf "$destination"; fi
        if [ "$new_link" = true ]; then rm -f "$link"; fi
        if [ -n "$backup" ] && [ -e "$backup" ]; then
            if ! mv "$backup" "$destination"; then
                printf 'Previous app retained at %s\n' "$backup" >&2
                rollback_ok=false
            fi
        fi
        if [ "$new_launcher" = true ]; then
            if [ -n "$launcher_backup" ]; then
                if ! mv -f "$launcher_backup" "$launcher"; then
                    printf 'Previous launcher retained at %s\n' "$launcher_backup" >&2
                    rollback_ok=false
                fi
            else
                rm -f "$launcher"
            fi
        fi
    fi
    if [ "$rollback_ok" = true ]; then
        [ -z "$staging" ] || rm -rf "$staging"
        [ -z "$launcher_backup" ] || rm -f "$launcher_backup"
    fi
    [ -z "$launcher_stage" ] || rm -f "$launcher_stage"
    rmdir "$lock"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
staging=$(mktemp -d "$parent/.tmux-workspaces-stage.XXXXXX")
if [ -n "$archive" ]; then
    [ -f "$archive" ] && [ -f "$archive.sha256" ] || fail 'The archive and its adjacent .sha256 file are required.'
    cp "$archive" "$staging/download.tar.gz"
    cp "$archive.sha256" "$staging/checksum"
else
    command -v curl >/dev/null 2>&1 || fail 'Install curl and retry.'
    base=https://github.com/eandualem/agent-interface/releases/latest/download
    name=tmux-workspaces-$target.tar.gz
    printf 'Downloading %s\n' "$name"
    curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 600 \
        "$base/$name" -o "$staging/download.tar.gz" || fail 'Download failed. Check that a release exists for this platform.'
    curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 60 \
        "$base/$name.sha256" -o "$staging/checksum"
fi
expected=$(awk 'NR == 1 {print $1}' "$staging/checksum")
[ ${#expected} -eq 64 ] || fail 'Invalid SHA-256 checksum.'
case $expected in *[!0-9a-f]*) fail 'Invalid SHA-256 checksum.';; esac
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$staging/download.tar.gz" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$staging/download.tar.gz" | awk '{print $1}')
fi
[ "$actual" = "$expected" ] || fail 'Archive checksum mismatch; nothing was installed.'
# Packages contain only regular files/directories inside a single known root.
tar -tzf "$staging/download.tar.gz" > "$staging/members"
awk -v root="$payload" '
    $0 != root && index($0, root "/") != 1 {exit 1}
    /(^|\/)\.\.(\/|$)/ {exit 1}
' "$staging/members" || fail 'Unsafe archive path.'
tar -tvzf "$staging/download.tar.gz" > "$staging/types"
awk 'substr($0,1,1) != "-" && substr($0,1,1) != "d" {exit 1}' "$staging/types" || fail 'Archive links or special files are not supported.'
tar -xzf "$staging/download.tar.gz" -C "$staging"
prepared=$staging/$payload
[ -f "$prepared/$marker" ] && [ "$(cat "$prepared/$marker")" = io.github.eandualem.tmux-workspaces ] || fail 'Invalid installation marker.'
[ "$(cat "$prepared/$(dirname "$marker")/TARGET")" = "$target" ] || fail 'The archive is for a different platform.'
if [ "$platform" = apple-darwin ]; then
    codesign --verify --strict "$prepared" || fail 'App signature verification failed.'
fi
"$prepared/$executable" --version || fail 'The binary cannot run on this system. Check the documented runtime requirements.'
if [ "$platform" = unknown-linux-gnu ]; then
    mkdir -p "$data/applications" "$HOME/.local/bin"
    # Stage beside the launcher so replacement stays atomic with a separate XDG volume.
    launcher_stage=$(mktemp "$data/applications/.tmux-workspaces-launcher.XXXXXX")
    if [ -f "$launcher" ]; then
        launcher_backup=$(mktemp "$data/applications/.tmux-workspaces-previous.XXXXXX")
        cp "$launcher" "$launcher_backup"
    fi
    # Exec has both desktop-string and command-argument escaping layers.
    escaped=$(printf '%s' "$destination/$executable" | sed 's/\\/\\\\\\\\/g; s/"/\\\\"/g; s/`/\\\\`/g; s/\$/\\\\$/g; s/%/%%/g')
    cat > "$launcher_stage" <<EOF
[Desktop Entry]
Type=Application
Name=Agent Interface
Comment=Organize tmux sessions into native workspaces
Exec="$escaped"
Icon=utilities-terminal
Terminal=false
Categories=Development;System;TerminalEmulator;
Keywords=tmux;terminal;workspace;
StartupWMClass=io.github.eandualem.tmux-workspaces
X-TmuxWorkspaces-Installer=1
EOF
fi
if [ -e "$destination" ]; then
    backup=$staging/previous
    mv "$destination" "$backup"
fi
mv "$prepared" "$destination"
installed=true
if [ "$platform" = unknown-linux-gnu ]; then
    # An existing owned symlink already points at the updated installation.
    if [ ! -L "$link" ]; then
        new_link=true
        ln -s "$destination/$executable" "$link"
    fi
    new_launcher=true
    mv -f "$launcher_stage" "$launcher"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
fi
printf '\nInstalled %s\n' "$destination"
if [ "$platform" = apple-darwin ]; then
    printf '%s\n' 'Open ~/Applications in Finder and launch tmux-workspaces.app.' \
        'Or run: open "$HOME/Applications/tmux-workspaces.app"' \
        'This app is ad-hoc signed, not notarized. If macOS blocks the first launch,' \
        'use System Settings > Privacy & Security > Open Anyway for this app.'
else
    printf '%s\n' 'Open Agent Interface from your applications menu.' \
        "For the command line, add $HOME/.local/bin to PATH if needed."
fi
if ! command -v tmux >/dev/null 2>&1 && [ ! -x /opt/homebrew/bin/tmux ] && [ ! -x /usr/local/bin/tmux ]; then
    printf '%s\n' 'Install tmux 3.3 or newer before opening terminal panes (see the installation guide).'
fi

}
main "$@"
