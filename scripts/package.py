#!/usr/bin/env python3
"""Build and package the native app for the host (Python is build-time only)."""

import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
APP_ID = "io.github.eandualem.agent-interface"


def output(*args, **kwargs):
    return subprocess.check_output(args, cwd=ROOT, text=True, **kwargs).strip()


def run(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def notices(destination, target, native, env):
    """Harvest the product graph, plus native code outside Cargo's graph."""
    if output("cargo-about", "--version") != "cargo-about 0.9.2":
        raise SystemExit("Packaging requires cargo-about 0.9.2 (see CONTRIBUTING.md).")
    graph = json.loads(output("cargo-about", "generate", "--manifest-path", "crates/app/Cargo.toml",
                              "--target", target, "--locked", "--fail", "--config", "packaging/about.toml",
                              "--format", "json", env=env))
    covered = set(json.loads((ROOT / "packaging/rust-fallbacks.json").read_text()))
    rust = ["Rust dependencies\n=================\n",
            "Unmodified crate sources, including MPL-2.0 option-ext: "
            "https://crates.io/api/v1/crates/NAME/VERSION/download\n"]
    for license in graph["licenses"]:
        members = [use["crate"]["name"] + "@" + use["crate"]["version"] for use in license["used_by"]]
        if not license.get("source_path") or "<copyright holders>" in license["text"]:
            missing = set(members) - covered
            if missing:
                raise SystemExit("Review missing licence notices: " + ", ".join(sorted(missing)))
            continue  # Complete notices below replace generic placeholder text.
        rust += [license["name"], ", ".join(members), license["text"]]
    rust.append((ROOT / "packaging/rust-notices.txt").read_text())
    if target.endswith("unknown-linux-gnu"):
        rust.append((ROOT / "packaging/wayland-notices.txt").read_text())
    (destination / "RUST-LICENSES.txt").write_text("\n\n".join(rust) + "\n")
    chunks = ["Native and embedded components\n==============================\n",
              (ROOT / "packaging/extra-notices.txt").read_text()]
    sources = json.loads((ROOT / "packaging/native-licenses.json").read_text())
    for source in sources:
        with urllib.request.urlopen(source["url"], timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != source["sha256"]:
            raise SystemExit("License source changed: " + source["name"])
        chunks += [source["name"] + "\n" + source["url"], data.decode("utf-8")]
    chunks += ["Ghostty (patched terminal core)\n" +
               "https://github.com/ghostty-org/ghostty/tree/" +
               json.loads((ROOT / "native/ghostty.json").read_text())["commit"],
               (native / "LICENSE").read_text(),
               "Lucide icons", (ROOT / "crates/app/assets/icons/LICENSE-lucide").read_text()]
    # Official Zig archives and the Homebrew layout place LICENSE differently.
    zig_exe = Path(shutil.which("zig")).resolve()
    zig_lib = next(p for p in (zig_exe.parent / "lib", zig_exe.parent.parent / "lib/zig") if (p / "std/std.zig").is_file())
    zig_license = next((p for p in (zig_exe.parent / "LICENSE", zig_exe.parent.parent / "LICENSE") if p.is_file()), None)
    if zig_license is None:
        raise SystemExit("Cannot find the installed Zig LICENSE.")
    chunks += ["Zig 0.15.2 compiler runtime", zig_license.read_text(),
               "Musl-derived runtime code", (zig_lib / "libc/musl/COPYRIGHT").read_text()]
    metadata = json.loads(output("cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target))
    sqlite = next(p for p in metadata["packages"] if p["name"] == "libsqlite3-sys")
    amalgamation = Path(sqlite["manifest_path"]).parent / "sqlite3/sqlite3.c"
    chunks += ["SQLite public-domain dedication", re.search(r"/\*\n\*\* 2001 September 15.*?\*/", amalgamation.read_text(), re.DOTALL).group()]
    (destination / "NATIVE-LICENSES.txt").write_text("\n\n".join(chunks) + "\n")
    rust_notice = Path(output("rustc", "--print", "sysroot")) / "share/doc/rust/COPYRIGHT-library.html"
    shutil.copyfile(rust_notice, destination / "RUST-STANDARD-LIBRARY.html")
    shutil.copyfile(ROOT / "LICENSE", destination / "LICENSE")


def main():
    os.chdir(ROOT)
    target = next(line.removeprefix("host: ") for line in output("rustc", "-vV").splitlines() if line.startswith("host: "))
    if target not in {f"{arch}-{platform}" for arch in ("aarch64", "x86_64") for platform in ("apple-darwin", "unknown-linux-gnu")}:
        raise SystemExit("Unsupported packaging host: " + target)
    if output("zig", "version") != "0.15.2":
        raise SystemExit("The native engine requires Zig 0.15.2.")
    native = Path(output("python3", "scripts/prepare-native.py"))
    env = dict(os.environ, LIBGHOSTTY_VT_SYS_OPTIMIZE="ReleaseFast",
               LIBGHOSTTY_VT_SYS_CPU="baseline", GHOSTTY_SOURCE_DIR=str(native),
               CARGO_INCREMENTAL="0")
    env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join([
        "--remap-path-prefix=" + str(Path.home()) + "=/build",
        "--remap-path-prefix=" + str(ROOT) + "=/src/agent-interface",
        "-Ctarget-cpu=generic",
    ])
    # Never infer optimization from Cargo's debug-info setting.
    build = subprocess.Popen(["cargo", "build", "--release", "--locked", "--target", target, "-p", "agent-interface-app",
                              "--bin", "agent-interface", "--message-format=json-render-diagnostics"],
                             cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True)
    binary = None
    for line in build.stdout:
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            binary = Path(message["executable"])
    if build.wait() != 0 or binary is None:
        raise SystemExit("Release build failed.")
    version = output(str(binary), "--version").removeprefix("agent-interface ")
    source = output("git", "rev-parse", "HEAD")
    dirty = bool(output("git", "status", "--porcelain", "--untracked-files=normal"))
    dist = ROOT / "dist"
    dist.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="package-", dir=dist) as temporary:
        stage = Path(temporary)
        mac = target.endswith("apple-darwin")
        bundle = stage / ("Agent Interface.app" if mac else "agent-interface")
        bin_dir = bundle / ("Contents/MacOS" if mac else "bin")
        resources = bundle / "Contents/Resources" if mac else bundle
        bin_dir.mkdir(parents=True)
        resources.mkdir(parents=True, exist_ok=True)
        executable = bin_dir / "agent-interface"
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        run("strip", "-x" if mac else "--strip-unneeded", str(executable))
        notices(resources, target, native, env)
        shutil.copyfile(ROOT / "scripts/setup_backbone.py", resources / "setup_backbone.py")
        shutil.copyfile(ROOT / "scripts/upgrade_backbone.py", resources / "upgrade_backbone.py")
        shutil.copyfile(ROOT / ("packaging/icon/AppIcon.icns" if mac else "packaging/icon/agent-interface.png"),
                        resources / ("AppIcon.icns" if mac else "agent-interface.png"))
        (resources / "INSTALLER").write_text(APP_ID + "\n")
        (resources / "TARGET").write_text(target + "\n")
        (resources / "BUILD.json").write_text(json.dumps({
            "version": version, "target": target, "source": source, "source_dirty": dirty,
            "rust": output("rustc", "--version"), "zig": "0.15.2",
            "profile": "release", "native_optimization": "ReleaseFast", "cpu": "baseline",
            "ghostty": json.loads((ROOT / "native/ghostty.json").read_text()),
        }, indent=2) + "\n")
        if mac:
            with (bundle / "Contents/Info.plist").open("wb") as stream:
                plistlib.dump({"CFBundleIdentifier": APP_ID, "CFBundleName": "Agent Interface",
                               "CFBundleExecutable": "agent-interface", "CFBundleIconFile": "AppIcon", "CFBundlePackageType": "APPL",
                               "CFBundleShortVersionString": version, "CFBundleVersion": version,
                               "LSMinimumSystemVersion": "11.0", "NSHighResolutionCapable": True}, stream)
            run("codesign", "--force", "--sign", "-", str(bundle))
            run("codesign", "--verify", "--strict", str(bundle))
            dependencies = output("otool", "-L", str(executable))
            if any(not line.strip().startswith(("/System/Library/", "/usr/lib/")) for line in dependencies.splitlines()[1:]):
                raise SystemExit("App links a non-system library:\n" + dependencies)
        else:
            # Keep the actual ABI and library requirements alongside the artifact.
            (resources / "ELF-REQUIREMENTS.txt").write_text(output("readelf", "-d", "--version-info", str(executable)) + "\n")
        archive = dist / f"agent-interface-{target}.tar.gz"
        with tarfile.open(archive, "w:gz", format=tarfile.USTAR_FORMAT) as tar:
            for path in sorted(bundle.rglob("*")):
                if path.is_symlink():
                    raise SystemExit("Packages must not contain symlinks.")
                info = tar.gettarinfo(str(path), arcname=str(path.relative_to(stage)))
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                info.mode = 0o755 if path.is_dir() or path == executable else 0o644
                if path.is_file():
                    with path.open("rb") as stream:
                        tar.addfile(info, stream)
                else:
                    tar.addfile(info)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        archive.with_suffix(archive.suffix + ".sha256").write_text(digest + "  " + archive.name + "\n")
        print(archive)
        print(digest)


if __name__ == "__main__":
    main()
