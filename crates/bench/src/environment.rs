//! The machine, software and build a benchmark ran on, as decision record
//! 0013 requires every result to state.

use std::process::Command;

/// Returns the environment as named values, in report order, with the
/// version a private tmux server reported. A value the system does not
/// provide is reported as `unknown`.
pub fn describe(tmux_version: &str) -> Vec<(&'static str, String)> {
    let engine = tmux_workspaces_term::engine_build();
    let mut values = vec![
        ("revision", env!("BENCH_REVISION").to_owned()),
        ("os", os()),
        ("cpu", cpu()),
        ("memory", memory()),
        ("power", power()),
        ("other builds running before", builds()),
        ("tmux", tmux_version.to_owned()),
        ("rustc", env!("BENCH_RUSTC").to_owned()),
        ("zig", env!("BENCH_ZIG").to_owned()),
        (
            "build profile",
            format!(
                "{} (opt-level {}, debug info {}, debug assertions {})",
                env!("BENCH_PROFILE"),
                env!("BENCH_OPT_LEVEL"),
                env!("BENCH_DEBUG"),
                cfg!(debug_assertions)
            ),
        ),
    ];
    match engine {
        Ok(engine) => values.extend([
            ("libghostty-vt", engine.version.to_owned()),
            ("libghostty-vt optimisation", engine.optimize),
            ("libghostty-vt SIMD", engine.simd.to_string()),
        ]),
        Err(error) => values.push(("libghostty-vt", error.to_string())),
    }
    values.extend([
        (
            "libghostty-vt CPU target",
            env!("BENCH_GHOSTTY_CPU").to_owned(),
        ),
        ("rio-vt", "0.5.28, default features off".to_owned()),
        ("alacritty_terminal", "0.26.0".to_owned()),
    ]);
    values
}

/// Returns the system's load averages now.
pub fn load() -> String {
    if cfg!(target_os = "macos") {
        run("sysctl", &["-n", "vm.loadavg"])
    } else {
        read("/proc/loadavg")
    }
}

/// Lists running compilers and build tools, which make a result exploratory.
pub fn builds() -> String {
    let processes = run("ps", &["-axo", "comm="]);
    let mut found: Vec<&str> = processes
        .lines()
        .filter_map(|path| path.trim().rsplit('/').next())
        .filter(|name| {
            [
                "cargo",
                "rustc",
                "zig",
                "clang",
                "cc1",
                "ld",
                "swift-frontend",
            ]
            .contains(name)
        })
        .collect();
    found.sort_unstable();
    found.dedup();
    if found.is_empty() {
        "none".to_owned()
    } else {
        found.join(", ")
    }
}

fn os() -> String {
    if cfg!(target_os = "macos") {
        format!(
            "{} {} ({})",
            run("sw_vers", &["-productName"]),
            run("sw_vers", &["-productVersion"]),
            run("sw_vers", &["-buildVersion"])
        )
    } else {
        let release = read("/etc/os-release");
        let name = release
            .lines()
            .find_map(|line| line.strip_prefix("PRETTY_NAME="))
            .map(|name| name.trim_matches('"').to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        format!("{name}, kernel {}", run("uname", &["-r"]))
    }
}

fn cpu() -> String {
    if cfg!(target_os = "macos") {
        format!(
            "{}, {} cores ({} performance, {} efficiency)",
            run("sysctl", &["-n", "machdep.cpu.brand_string"]),
            run("sysctl", &["-n", "hw.ncpu"]),
            run("sysctl", &["-n", "hw.perflevel0.logicalcpu"]),
            run("sysctl", &["-n", "hw.perflevel1.logicalcpu"])
        )
    } else {
        let info = read("/proc/cpuinfo");
        let model = info
            .lines()
            .find_map(|line| line.strip_prefix("model name"))
            .map(|rest| rest.trim_start_matches([' ', '\t', ':']).to_owned())
            .unwrap_or_else(|| "unknown".to_owned());
        format!("{model}, {} cores", run("nproc", &[]))
    }
}

fn memory() -> String {
    if cfg!(target_os = "macos") {
        let bytes: u64 = run("sysctl", &["-n", "hw.memsize"]).parse().unwrap_or(0);
        format!("{} GiB", bytes >> 30)
    } else {
        read("/proc/meminfo")
            .lines()
            .next()
            .unwrap_or("unknown")
            .to_owned()
    }
}

fn power() -> String {
    if cfg!(target_os = "macos") {
        let source = run("pmset", &["-g", "batt"]);
        let source = source.lines().next().unwrap_or("unknown").to_owned();
        let settings = run("pmset", &["-g"]);
        let low_power = settings
            .lines()
            .find(|line| line.trim_start().starts_with("lowpowermode"))
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_else(|| "lowpowermode unknown".to_owned());
        format!("{source}; {low_power}")
    } else {
        "unknown".to_owned()
    }
}

fn run(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned())
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned())
}
