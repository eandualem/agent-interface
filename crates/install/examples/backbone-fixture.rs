//! The installer tests' stand-in for the app binary's Backbone commands,
//! with a release key the tests make. Never part of a package.
//!
//! - `keygen DIRECTORY` makes a key pair, as `DIRECTORY/public` (the base64
//!   line of the public key) and `DIRECTORY/secret`.
//! - `sign DIRECTORY FILE` writes the signature of `FILE` to `FILE.minisig`.
//! - `backbone-install ARGUMENTS` runs the app's command, trusting the key
//!   in `BACKBONE_FIXTURE_KEY`. An empty value stands for a build with no
//!   release key. Without it, it runs exactly what a shipped app runs.

use std::{ffi::OsString, fs, path::PathBuf, process::ExitCode};

use minisign::{KeyPair, SecretKeyBox};

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let command = arguments.next();
    let mut path = || PathBuf::from(arguments.next().expect("a path"));
    match command.as_ref().and_then(|command| command.to_str()) {
        Some("keygen") => {
            let directory = path();
            let keys = KeyPair::generate_unencrypted_keypair().unwrap();
            fs::write(directory.join("public"), keys.pk.to_base64()).unwrap();
            let secret = keys.sk.to_box(None).unwrap().into_string();
            fs::write(directory.join("secret"), secret).unwrap();
        }
        Some("sign") => {
            let (directory, file) = (path(), path());
            let secret = fs::read_to_string(directory.join("secret")).unwrap();
            let secret = SecretKeyBox::from_string(&secret)
                .unwrap()
                .into_unencrypted_secret_key()
                .unwrap();
            let data = fs::read(&file).unwrap();
            let signature = minisign::sign(None, &secret, &data[..], None, None).unwrap();
            let mut name = OsString::from(file.as_os_str());
            name.push(".minisig");
            fs::write(name, signature.into_string()).unwrap();
        }
        Some("backbone-install") => {
            // An empty key stands for a build that carries no release key.
            return match std::env::var("BACKBONE_FIXTURE_KEY") {
                Ok(key) if key.is_empty() => agent_interface_install::run(arguments, None),
                Ok(key) => agent_interface_install::run(arguments, Some(&key)),
                Err(_) => agent_interface_install::main(arguments),
            };
        }
        _ => {
            eprintln!(
                "Usage: backbone-fixture keygen DIRECTORY | sign DIRECTORY FILE | backbone-install ARGUMENTS"
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
