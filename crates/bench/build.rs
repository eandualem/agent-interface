//! Records how the benchmark was built, for its reports.
//!
//! `tmux-workspaces-bench-rio` uses this build script too, and so does the app
//! package, whose benchmark window states the revision it was built from.

use std::env;
use std::path::Path;
use std::process::Command;

/// The paths whose contents the benchmark programs are built from.
const SOURCES: [&str; 4] = ["crates", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml"];

fn main() {
    // The revision the programs were built from, so that a report names the
    // code that ran, wherever and whenever it runs.
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("Cargo sets CARGO_MANIFEST_DIR");
    let root = Path::new(&manifest).join("../..");
    for source in SOURCES {
        println!("cargo:rerun-if-changed={}", root.join(source).display());
    }
    let reference = git(&root, &["rev-parse", "--symbolic-full-name", "HEAD"]);
    for name in ["HEAD", "index", reference.as_str()] {
        let path = git(&root, &["rev-parse", "--git-path", name]);
        if !path.is_empty() {
            println!("cargo:rerun-if-changed={}", root.join(path).display());
        }
    }
    let commit = git(&root, &["rev-parse", "HEAD"]);
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend(SOURCES);
    let changed = !git(&root, &status).is_empty();
    println!(
        "cargo:rustc-env=BENCH_REVISION={}{}",
        if commit.is_empty() {
            "unknown"
        } else {
            &commit
        },
        if changed {
            " with uncommitted changes"
        } else {
            ""
        }
    );

    // The engine's build script reads the same variables, in the same Cargo
    // invocation, to choose its CPU target and optimisation mode.
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_CPU");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    let cpu = env::var("LIBGHOSTTY_VT_SYS_CPU").unwrap_or_else(|_| "baseline".to_owned());
    println!("cargo:rustc-env=BENCH_GHOSTTY_CPU={cpu}");

    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    println!("cargo:rustc-env=BENCH_RUSTC={}", version(&rustc, "-V"));
    // The engine's build script runs the `zig` it finds on `PATH`.
    println!("cargo:rerun-if-env-changed=PATH");
    println!("cargo:rustc-env=BENCH_ZIG={}", version("zig", "version"));
    for (name, variable) in [
        ("BENCH_PROFILE", "PROFILE"),
        ("BENCH_OPT_LEVEL", "OPT_LEVEL"),
        ("BENCH_DEBUG", "DEBUG"),
    ] {
        println!(
            "cargo:rustc-env={name}={}",
            env::var(variable).unwrap_or_default()
        );
    }
}

fn version(program: &str, arg: &str) -> String {
    Command::new(program)
        .arg(arg)
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// Runs `git` in `root` and returns its trimmed output, or nothing if it fails.
fn git(root: &Path, args: &[&str]) -> String {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_default()
}
