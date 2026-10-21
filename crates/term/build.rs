//! Refuse native sources that lack the patches the Rust terminal path requires.

use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let manifest_path = root.join("native/ghostty.json");
    let patch_path = root.join("native/ghostty.patch");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    println!("cargo:rerun-if-changed={}", patch_path.display());
    println!("cargo:rerun-if-env-changed=GHOSTTY_SOURCE_DIR");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read native manifest"))
            .expect("parse native manifest");
    assert_eq!(
        hash(&patch_path),
        manifest["patch_sha256"].as_str().expect("patch digest"),
        "native patch and manifest disagree"
    );
    println!(
        "cargo:rustc-env=TMUX_GHOSTTY_PATCH_SHA256={}",
        manifest["patch_sha256"].as_str().expect("patch digest")
    );
    // The binding also reads this as Unicode. An OsString could verify a
    // different source from the one the binding actually builds.
    let source = PathBuf::from(env::var("GHOSTTY_SOURCE_DIR").expect(
        "GHOSTTY_SOURCE_DIR must be a UTF-8 path; run python3 scripts/prepare-native.py from the repository",
    ));
    let directory = format!(
        "ghostty-{}-{}",
        manifest["commit"].as_str().expect("native commit"),
        manifest["patch_sha256"].as_str().expect("patch digest")
    );
    assert!(
        source.is_absolute() && source.file_name() == Some(std::ffi::OsStr::new(&directory)),
        "GHOSTTY_SOURCE_DIR must be an absolute, immutable source path ending in {directory}; prepare a new directory instead of patching an existing override"
    );
    for (name, digest) in manifest["files"].as_object().expect("native file digests") {
        let path = source.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        assert_eq!(
            hash(&path),
            digest.as_str().expect("native file digest"),
            "native source lacks the required patches: {}; run python3 scripts/prepare-native.py",
            path.display()
        );
    }
}

fn hash(path: &std::path::Path) -> String {
    let bytes = fs::read(path).unwrap_or_else(|error| {
        panic!(
            "cannot read native build input {}: {error}; run python3 scripts/prepare-native.py",
            path.display()
        )
    });
    format!("{:x}", Sha256::digest(bytes))
}
