//! Records how the benchmark was built, for its reports.

use std::env;
use std::process::Command;

fn main() {
    // The engine's build script reads the same variables, in the same Cargo
    // invocation, to choose its CPU target and optimisation mode.
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_CPU");
    println!("cargo:rerun-if-env-changed=LIBGHOSTTY_VT_SYS_OPTIMIZE");
    let cpu = env::var("LIBGHOSTTY_VT_SYS_CPU").unwrap_or_else(|_| "baseline".to_owned());
    println!("cargo:rustc-env=BENCH_GHOSTTY_CPU={cpu}");

    let rustc = env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let version = Command::new(rustc)
        .arg("-V")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_default();
    println!("cargo:rustc-env=BENCH_RUSTC={}", version.trim());
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
