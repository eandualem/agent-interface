#!/bin/sh
# Install the native desktop app for the current user. No build tools.
set -eu

# Wait for the complete script before changing anything when piped from curl.
main() {

fail() { printf 'Agent Interface: %s\n' "$*" >&2; exit 1; }
progress_pid=
stop_progress() {
    if [ -n "$progress_pid" ]; then
        kill "$progress_pid" 2>/dev/null || true
        wait "$progress_pid" 2>/dev/null || true
        progress_pid=
    fi
}
run_step() {
    step_title=$1
    shift
    printf '%s...\n' "$step_title" >&2
    (
        tick=
        trap '[ -z "$tick" ] || { kill "$tick" 2>/dev/null || :; wait "$tick" 2>/dev/null || :; }; exit 0' HUP INT TERM
        elapsed=0
        while :; do
            sleep 5 &
            tick=$!
            wait "$tick" || exit 0
            elapsed=$((elapsed + 5))
            printf '  %s: still running (%ss elapsed)\n' "$step_title" "$elapsed" >&2
        done
    ) &
    progress_pid=$!
    if "$@"; then step_status=0; else step_status=$?; fi
    stop_progress
    if [ "$step_status" -eq 0 ]; then
        printf '%s: done.\n' "$step_title" >&2
    else
        printf '%s: failed.\n' "$step_title" >&2
    fi
    return "$step_status"
}
usage() {
    printf '%s\n' 'Usage: sh install.sh [--archive PATH]' \
        'Installs the latest published build, or a local archive with its .sha256 file.' \
        'Quit the app before updating. Layouts and settings in the agent-interface folders are preserved.'
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
wsl_launcher=
wsl_launcher_backup=
new_wsl_launcher=false
if [ "$platform" = apple-darwin ]; then
    parent=$HOME/Applications
    destination="$parent/Agent Interface.app"
    payload='Agent Interface.app'
    executable=Contents/MacOS/agent-interface
    marker=Contents/Resources/INSTALLER
else
    getconf GNU_LIBC_VERSION >/dev/null 2>&1 || fail 'This build requires glibc Linux (not musl).'
    parent=$HOME/.local/lib
    destination=$parent/agent-interface
    payload=agent-interface
    executable=bin/agent-interface
    marker=INSTALLER
    data=${XDG_DATA_HOME:-$HOME/.local/share}
    case $data in /*) ;; *) fail 'XDG_DATA_HOME must be an absolute path.';; esac
    launcher=$data/applications/io.github.eandualem.agent-interface.desktop
    link=$HOME/.local/bin/agent-interface
    if [ -n "${WSL_DISTRO_NAME:-}" ]; then
        # WSLg scans system application directories, not XDG_DATA_HOME.
        # Keep the desktop ID aligned with the window ID for taskbar grouping.
        wsl_launcher=/usr/share/applications/io.github.eandualem.agent-interface.desktop
        if [ -e "$wsl_launcher" ] || [ -L "$wsl_launcher" ]; then
            [ ! -L "$wsl_launcher" ] && [ -f "$wsl_launcher" ] &&
                grep -qx 'X-AgentInterface-Installer=1' "$wsl_launcher" &&
                grep -Fqx "X-AgentInterface-Home=$HOME" "$wsl_launcher" ||
                fail "Refusing to replace $wsl_launcher."
        fi
    fi
    if [ -e "$launcher" ] || [ -L "$launcher" ]; then
        [ -f "$launcher" ] && grep -qx 'X-AgentInterface-Installer=1' "$launcher" || fail "Refusing to replace $launcher."
    fi
    if [ -e "$link" ] || [ -L "$link" ]; then
        [ -L "$link" ] && [ "$(readlink "$link")" = "$destination/bin/agent-interface" ] ||
            fail "Refusing to replace $link."
    fi
fi
if [ -e "$destination" ] || [ -L "$destination" ]; then
    [ ! -L "$destination" ] && [ -f "$destination/$marker" ] &&
        [ "$(cat "$destination/$marker")" = io.github.eandualem.agent-interface ] ||
        fail "Refusing to replace an installation not owned by this installer: $destination"
fi
mkdir -p "$parent"
lock=$parent/.agent-interface-install.lock
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
    stop_progress
    rollback_ok=true
    if [ "$status" -ne 0 ]; then
        if [ "$new_wsl_launcher" = true ]; then
            if [ -n "$wsl_launcher_backup" ]; then
                if ! sudo install -m 644 -- "$wsl_launcher_backup" "$wsl_launcher"; then
                    printf 'Previous Windows launcher retained at %s\n' "$wsl_launcher_backup" >&2
                    rollback_ok=false
                fi
            elif ! sudo rm -f -- "$wsl_launcher"; then
                printf 'Could not remove Windows launcher %s\n' "$wsl_launcher" >&2
                rollback_ok=false
            fi
        fi
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
staging=$(mktemp -d "$parent/.agent-interface-stage.XXXXXX")
if [ -n "$wsl_launcher" ]; then
    [ -n "${WAYLAND_DISPLAY:-}${DISPLAY:-}" ] ||
        fail 'This installation needs WSL 2 with GUI support (WSLg). See the Windows installation guide.'
    command -v apt-get >/dev/null 2>&1 && command -v dpkg-query >/dev/null 2>&1 ||
        fail 'Automatic WSL setup requires Ubuntu 24.04 or newer.'
    command -v sudo >/dev/null 2>&1 || fail 'WSL setup requires sudo for runtime packages and the Windows launcher.'
fi
if [ -n "$archive" ]; then
    [ -f "$archive" ] && [ -f "$archive.sha256" ] || fail 'The archive and its adjacent .sha256 file are required.'
    cp "$archive" "$staging/download.tar.gz"
    cp "$archive.sha256" "$staging/checksum"
else
    command -v curl >/dev/null 2>&1 || fail 'Install curl and retry.'
    repository=https://github.com/eandualem/agent-interface
    printf 'Finding the latest release...\n'
    release=$(curl --fail --head --location --silent --show-error --proto '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 60 --retry 4 --retry-delay 2 --retry-connrefused \
        --write-out '%{url_effective}' "$repository/releases/latest" -o /dev/null) ||
        fail 'Could not find the latest release. Check your connection and retry.'
    case $release in
        "$repository/releases/tag/"?*) base=$repository/releases/download/${release#"$repository/releases/tag/"} ;;
        *) fail 'The latest release did not resolve to a release tag.' ;;
    esac
    name=agent-interface-$target.tar.gz
    # Fetch the small checksum first so a cached partial belongs to exactly
    # these bytes, even if the latest release changes between installer runs.
    printf 'Checking the release checksum...\n'
    curl --fail --location --silent --show-error --proto '=https' --tlsv1.2 \
        --connect-timeout 15 --max-time 60 --retry 4 --retry-delay 2 --retry-connrefused \
        "$base/$name.sha256" -o "$staging/checksum" ||
        fail 'Could not fetch the release checksum. Check your connection and retry.'
    expected=$(awk 'NR == 1 {print $1}' "$staging/checksum")
    [ ${#expected} -eq 64 ] || fail 'Invalid SHA-256 checksum.'
    case $expected in *[!0-9a-f]*) fail 'Invalid SHA-256 checksum.';; esac
    # The existing install lock also serializes this private, persistent cache.
    cache=$parent/.agent-interface-download
    [ ! -L "$cache" ] || fail 'Refusing a symlink download cache.'
    (umask 077; mkdir -p "$cache")
    chmod 700 "$cache"
    for file in "$cache/checksum" "$cache/archive.part"; do
        [ ! -L "$file" ] && { [ ! -e "$file" ] || [ -f "$file" ]; } ||
            fail 'Refusing a non-regular download cache file.'
    done
    if [ "$(cat "$cache/checksum" 2>/dev/null || true)" != "$expected" ]; then
        rm -f "$cache/archive.part"
        printf '%s\n' "$expected" > "$cache/checksum"
    fi
    download=$cache/archive.part
    download_matches() {
        [ -f "$download" ] || return 1
        if command -v sha256sum >/dev/null 2>&1; then
            digest=$(sha256sum "$download" | awk '{print $1}')
        else
            digest=$(shasum -a 256 "$download" | awk '{print $1}')
        fi
        [ "$digest" = "$expected" ]
    }
    attempt=1
    delay=2
    while ! download_matches; do
        bytes=0
        if [ -f "$download" ]; then bytes=$(wc -c < "$download" | tr -d ' '); fi
        printf 'Downloading %s (attempt %s/5; %s bytes saved)...\n' "$name" "$attempt" "$bytes" >&2
        # Each new curl process resumes the bytes from the last attempt. A
        # slow but active transfer has no total deadline; only stalls time out.
        if status=$(curl --fail --location --progress-bar --show-error --proto '=https' --tlsv1.2 \
            --connect-timeout 15 --speed-limit 1 --speed-time 60 --continue-at - \
            --write-out '%{http_code}' "$base/$name" -o "$download"); then
            transfer_status=0
        else
            transfer_status=$?
        fi
        download_matches && break
        if [ "$transfer_status" -eq 33 ] || [ "$status" = 416 ]; then
            # curl may report success for 416, assuming the local file is
            # complete. We already checked its hash, so restart if it differs.
            printf '%s\n' 'Server could not resume this file; retrying from the beginning.' >&2
            rm -f "$download"
        elif [ "$transfer_status" -eq 0 ]; then
            printf '%s\n' 'Checksum mismatch; retrying with a fresh archive.' >&2
            rm -f "$download"
        else
            case $transfer_status:$status in
                22:408|22:429|22:5??|5:*|6:*|7:*|16:*|18:*|28:*|35:*|52:*|55:*|56:*|92:*) ;;
                *) fail "Download failed (curl $transfer_status, HTTP $status); rerun the installer to resume any saved bytes in $cache." ;;
            esac
        fi
        [ "$attempt" -lt 5 ] || fail "Download could not finish after 5 attempts; rerun the installer to resume any saved bytes in $cache."
        printf 'Retrying in %ss; downloaded bytes are kept unless the server cannot resume or verification fails.\n' "$delay" >&2
        sleep "$delay"
        attempt=$((attempt + 1))
        delay=$((delay * 2))
    done
    cp "$download" "$staging/download.tar.gz"
fi
printf '%s\n' 'Verifying and unpacking the app...'
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
[ -f "$prepared/$marker" ] && [ "$(cat "$prepared/$marker")" = io.github.eandualem.agent-interface ] ||
    fail 'Invalid installation marker.'
[ "$(cat "$prepared/$(dirname "$marker")/TARGET")" = "$target" ] || fail 'The archive is for a different platform.'
if [ "$platform" = apple-darwin ]; then
    codesign --verify --strict "$prepared" || fail 'App signature verification failed.'
fi
if [ -n "$wsl_launcher" ]; then
    printf '%s\n' 'Preparing WSL runtime packages and Windows Start menu entry. sudo may ask for your Linux password.'
    sudo -v || fail 'Could not authorize WSL runtime and launcher installation.'
    missing=
    for package in tmux git fonts-dejavu-core libfontconfig1 libfreetype6 libxcb1 libxkbcommon0 \
        libxkbcommon-x11-0 libwayland-client0 libvulkan1 libegl1 libgl1-mesa-dri \
        mesa-vulkan-drivers libwebkit2gtk-4.1-0 xwayland xdg-desktop-portal xdg-desktop-portal-gtk; do
        if [ "$(dpkg-query -W -f='${Status}' "$package" 2>/dev/null || true)" != 'install ok installed' ]; then
            missing="$missing $package"
        fi
    done
    if [ -n "$missing" ]; then
        run_step 'Refreshing Ubuntu package information' sudo apt-get update || fail 'Could not refresh Ubuntu package information.'
        # The list contains only the fixed package names above.
        # shellcheck disable=SC2086
        run_step 'Installing WSL runtime packages' sudo apt-get install -y $missing || fail 'Could not install the WSL runtime packages.'
    fi
    if [ -f "$wsl_launcher" ]; then
        wsl_launcher_backup=$staging/previous-wsl-launcher
        cp "$wsl_launcher" "$wsl_launcher_backup"
    fi
fi
"$prepared/$executable" --version || fail 'The binary cannot run on this system. Check the documented runtime requirements.'
# Use the distribution's root-owned Bubblewrap policy when its sandbox cannot
# start. Never grant userns to writable app paths or replace an existing policy.
configure_linux_sandbox() {
    [ "$platform" = unknown-linux-gnu ] || return 0
    [ "$(cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns 2>/dev/null || true)" = 1 ] || return 0
    [ -d /sys/kernel/security/apparmor ] || return 0
    if timeout 5 /usr/bin/bwrap --ro-bind / / --proc /proc --dev /dev --unshare-all -- /bin/true >/dev/null 2>&1; then return 0; fi
    sandbox_warning='Documents need Linux sandbox support; native terminals and other views remain available. See the Linux installation guide.'
    sandbox_source=/usr/share/apparmor/extra-profiles/bwrap-userns-restrict
    sandbox_profile=/etc/apparmor.d/bwrap-userns-restrict
    if [ -e "$sandbox_profile" ] || [ -L "$sandbox_profile" ]; then
        printf 'Agent Interface: Existing Bubblewrap policy preserved. %s\n' "$sandbox_warning" >&2
        return 0
    fi
    printf '%s\n' 'Preparing distribution Bubblewrap sandbox support. sudo may ask for your Linux password.'
    if ! sudo -v; then printf 'Agent Interface: %s\n' "$sandbox_warning" >&2; return 0; fi
    if ! sudo test -r /sys/kernel/security/apparmor/profiles ||
        sudo grep -Eq '^(bwrap|unpriv_bwrap) \(' /sys/kernel/security/apparmor/profiles; then
        printf 'Agent Interface: Existing or unreadable sandbox policy preserved. %s\n' "$sandbox_warning" >&2
        return 0
    fi
    if [ ! -f "$sandbox_source" ]; then
        if ! command -v apt-get >/dev/null 2>&1 ||
            ! run_step 'Refreshing sandbox package information' sudo apt-get update ||
            ! run_step 'Installing distribution sandbox policy' sudo apt-get install -y apparmor-profiles; then
            printf 'Agent Interface: %s\n' "$sandbox_warning" >&2
            return 0
        fi
    fi
    if [ ! -f "$sandbox_source" ] || [ ! -x /usr/sbin/apparmor_parser ] ||
        ! run_step 'Loading distribution sandbox policy' sudo /usr/sbin/apparmor_parser -r "$sandbox_source"; then
        printf 'Agent Interface: Could not load the distribution sandbox policy. %s\n' "$sandbox_warning" >&2
        return 0
    fi
    if ! sudo install -m 644 -- "$sandbox_source" "$sandbox_profile"; then
        printf 'Agent Interface: Sandbox policy is active but could not be saved for restart.\n' >&2
    fi
}
configure_linux_sandbox
printf '%s\n' 'Installing the app and its launcher...'
if [ "$platform" = unknown-linux-gnu ]; then
    mkdir -p "$data/applications" "$HOME/.local/bin"
    # Stage beside the launcher so replacement stays atomic with a separate XDG volume.
    launcher_stage=$(mktemp "$data/applications/.agent-interface-launcher.XXXXXX")
    if [ -f "$launcher" ]; then
        launcher_backup=$(mktemp "$data/applications/.agent-interface-previous.XXXXXX")
        cp "$launcher" "$launcher_backup"
    fi
    # Exec has both desktop-string and command-argument escaping layers.
    escaped=$(printf '%s' "$destination/$executable" | sed 's/\\/\\\\\\\\/g; s/"/\\\\"/g; s/`/\\\\`/g; s/\$/\\\\$/g; s/%/%%/g')
    icon=$(printf '%s' "$destination/agent-interface.png" | sed 's/\\/\\\\/g')
    cat > "$launcher_stage" <<EOF
[Desktop Entry]
Type=Application
Name=Agent Interface
Comment=Organize tmux sessions into native workspaces
Exec="$escaped"
Icon=$icon
Terminal=false
Categories=Development;System;TerminalEmulator;
Keywords=tmux;terminal;workspace;
StartupWMClass=$(cat "$prepared/$marker")
X-AgentInterface-Installer=1
X-AgentInterface-Home=$HOME
EOF
fi
if [ -e "$destination" ]; then
    backup=$staging/previous
    mv "$destination" "$backup"
fi
mv "$prepared" "$destination"
installed=true
if [ "$platform" = unknown-linux-gnu ]; then
    if [ ! -L "$link" ]; then
        new_link=true
        ln -s "$destination/$executable" "$link"
    fi
    new_launcher=true
    mv -f "$launcher_stage" "$launcher"
    chmod 644 "$launcher"
    if [ -n "$wsl_launcher" ]; then
        new_wsl_launcher=true
        sudo install -m 644 -- "$launcher" "$wsl_launcher" ||
            fail 'Could not register Agent Interface in Windows Start. The app update will be rolled back.'
    fi
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
fi
printf '\nInstalled %s\n' "$destination"
if [ "$platform" = apple-darwin ]; then
    printf '%s\n' 'Open ~/Applications in Finder and launch Agent Interface.app.' \
        'Or run: open "$HOME/Applications/Agent Interface.app"' \
        'This app is ad-hoc signed, not notarized. If macOS blocks the first launch,' \
        'use System Settings > Privacy & Security > Open Anyway for this app.'
else
    if [ -n "$wsl_launcher" ]; then
        printf '%s\n' "Open Agent Interface under $WSL_DISTRO_NAME in Windows Start."
    else
        printf '%s\n' 'Open Agent Interface from your applications menu.'
    fi
    printf 'Or run: "%s/.local/bin/agent-interface"\n' "$HOME"
fi
if ! command -v tmux >/dev/null 2>&1 && [ ! -x /opt/homebrew/bin/tmux ] && [ ! -x /usr/local/bin/tmux ]; then
    printf '%s\n' 'Install tmux 3.4 or newer before opening terminal panes (see the installation guide).'
fi

}
main "$@"
