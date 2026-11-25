//! The commands `install.sh` runs, through the app binary, to install Agent
//! Backbone from its signed release.
//!
//! The installer downloads every file with curl; these commands make every
//! check, through the vendored `backbone-release` crate, and hold no state
//! between runs:
//!
//! 1. `check` says whether Backbone may be installed at all: the build has
//!    a release key, and no earlier Backbone is in the way.
//! 2. `verify-manifest` verifies the downloaded manifest and its signature
//!    against the release key and the installed version, and prints the
//!    release's version and this target's archive name.
//! 3. `install` verifies the manifest again, checks the archive's digest,
//!    unpacks `backbone` and puts it in `~/.local/bin` by a rename in that
//!    directory.
//!
//! The installer owns `~/.local/bin/backbone` while a marker it writes
//! beside it exists, also once `backbone upgrade` has replaced the program.
//! Only an owned program is run, with `--version`, to learn the installed
//! version. Each refusal has its own exit status, listed in [`status`].

use std::{
    ffi::OsString,
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{ExitCode, Stdio},
    thread,
    time::{Duration, Instant},
};

use backbone_release::{Refusal, TrustedKey, VerifiedManifest};
use semver::Version;

/// The release key's public half, the base64 line of its minisign `.pub`
/// file, or `None` while no key is made: a build without it trusts no
/// release and installs no Backbone.
pub const RELEASE_KEY: Option<&str> = None;

/// The exit statuses the installer tells apart.
pub mod status {
    /// Backbone could not be installed, for a reason of this computer.
    pub const FAILED: u8 = 1;
    /// The command was not given as [`USAGE`](super::USAGE) shows.
    pub const USAGE: u8 = 2;
    /// This build has no release key.
    pub const NOT_PUBLISHED: u8 = 3;
    /// An earlier Backbone is installed, and was left in place.
    pub const EARLIER: u8 = 4;
    /// The manifest or its signature is missing or does not hold, as it
    /// can while the latest release is being replaced: download both again.
    pub const RETRY: u8 = 5;
    /// The release was refused.
    pub const REFUSED: u8 = 6;
}

/// How the commands are given, after the app's own `backbone-install`.
pub const USAGE: &str = "Usage: agent-interface backbone-install check\n       agent-interface backbone-install verify-manifest --target TARGET MANIFEST SIGNATURE\n       agent-interface backbone-install install --target TARGET MANIFEST SIGNATURE ARCHIVE\n";

/// What the installer says when an earlier Backbone is in the way.
const EARLIER: &str = "An earlier Agent Backbone was found and left in place.";

/// The marker beside `backbone` that says this installer put it there. It
/// also records the version placed, for whoever reads it; nothing does.
const MARKER: &str = ".agent-interface-backbone";
const MARKER_HEADER: &str = "agent-interface backbone 1";

/// Runs the command `arguments` give, trusting the release key built into
/// this build. The app binary calls this.
pub fn main(arguments: impl IntoIterator<Item = OsString>) -> ExitCode {
    run(arguments, RELEASE_KEY)
}

/// Runs the command `arguments` give, trusting `key`. Only [`main`] passes
/// the key of a shipped build; the installer tests' example passes theirs.
pub fn run(arguments: impl IntoIterator<Item = OsString>, key: Option<&str>) -> ExitCode {
    let result = parse(arguments).and_then(|command| execute(&command, key, &Places::from_env()?));
    match result {
        Ok(output) => {
            print!("{output}");
            ExitCode::SUCCESS
        }
        Err(failure) => {
            eprintln!("{failure}");
            ExitCode::from(failure.status())
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Check,
    VerifyManifest {
        target: String,
        manifest: PathBuf,
        signature: PathBuf,
    },
    Install {
        target: String,
        manifest: PathBuf,
        signature: PathBuf,
        archive: PathBuf,
    },
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Usage(String),
    NotPublished,
    Earlier,
    Retry(String),
    Refused(String),
    Failed(String),
}

impl Failure {
    fn status(&self) -> u8 {
        match self {
            Failure::Usage(_) => status::USAGE,
            Failure::NotPublished => status::NOT_PUBLISHED,
            Failure::Earlier => status::EARLIER,
            Failure::Retry(_) => status::RETRY,
            Failure::Refused(_) => status::REFUSED,
            Failure::Failed(_) => status::FAILED,
        }
    }

    fn failed(what: impl fmt::Display) -> Failure {
        Failure::Failed(format!("Agent Backbone could not be installed: {what}."))
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Failure::Usage(why) => write!(f, "backbone-install: {why}\n{USAGE}"),
            Failure::NotPublished => {
                f.write_str("Agent Backbone is not published yet, so it was not installed.")
            }
            Failure::Earlier => f.write_str(EARLIER),
            Failure::Retry(why) | Failure::Refused(why) | Failure::Failed(why) => f.write_str(why),
        }
    }
}

fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Command, Failure> {
    let usage = |why: &str| Failure::Usage(why.to_owned());
    let mut arguments = arguments.into_iter();
    let command = arguments
        .next()
        .ok_or_else(|| usage("a command is required"))?;
    let (mut target, mut files) = (None, Vec::new());
    while let Some(argument) = arguments.next() {
        if argument == "--target" && target.is_none() {
            target = Some(
                arguments
                    .next()
                    .and_then(|target| target.into_string().ok())
                    .filter(|target| !target.is_empty())
                    .ok_or_else(|| usage("--target needs a target"))?,
            );
        } else if argument.as_encoded_bytes().starts_with(b"-") {
            return Err(usage(&format!(
                "unknown option {}",
                argument.to_string_lossy()
            )));
        } else {
            files.push(PathBuf::from(argument));
        }
    }
    let required = || target.clone().ok_or_else(|| usage("--target is required"));
    match (command.to_str(), &mut files[..]) {
        (Some("check"), []) if target.is_none() => Ok(Command::Check),
        (Some("verify-manifest"), [manifest, signature]) => Ok(Command::VerifyManifest {
            target: required()?,
            manifest: std::mem::take(manifest),
            signature: std::mem::take(signature),
        }),
        (Some("install"), [manifest, signature, archive]) => Ok(Command::Install {
            target: required()?,
            manifest: std::mem::take(manifest),
            signature: std::mem::take(signature),
            archive: std::mem::take(archive),
        }),
        (Some("check" | "verify-manifest" | "install"), _) => {
            Err(usage("the command's files are not as shown"))
        }
        _ => Err(usage(&format!(
            "unknown command {}",
            command.to_string_lossy()
        ))),
    }
}

/// Where Backbone goes, and where an earlier Backbone's login service is
/// defined.
struct Places {
    bin: PathBuf,
    earlier_services: [PathBuf; 2],
}

impl Places {
    fn from_env() -> Result<Places, Failure> {
        let absolute = |name| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        };
        let home =
            absolute("HOME").ok_or_else(|| Failure::failed("HOME is not an absolute path"))?;
        let config = absolute("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        Ok(Places::new(&home, &config))
    }

    fn new(home: &Path, config: &Path) -> Places {
        Places {
            bin: home.join(".local/bin"),
            // The earlier, Python service's, on macOS and on Linux. The Rust
            // service's are named agent-backbone-rs and are not in the way.
            earlier_services: [
                home.join("Library/LaunchAgents/dev.agent-backbone.plist"),
                config.join("systemd/user/agent-backbone.service"),
            ],
        }
    }

    fn program(&self) -> PathBuf {
        self.bin.join("backbone")
    }

    fn marker(&self) -> PathBuf {
        self.bin.join(MARKER)
    }
}

fn execute(command: &Command, key: Option<&str>, places: &Places) -> Result<String, Failure> {
    match command {
        Command::Check => {
            trusted(key)?;
            installed(places)?;
            Ok(String::new())
        }
        Command::VerifyManifest {
            target,
            manifest,
            signature,
        } => {
            let manifest = verified(key, places, manifest, signature)?;
            let name = manifest.artifact(target).map_err(refused)?;
            // The installer puts the name in a download URL.
            if name.starts_with('.')
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._+-".contains(&byte))
            {
                return Err(Failure::Refused(format!(
                    "Agent Backbone's release was refused: its archive name {name:?} is not a plain file name."
                )));
            }
            Ok(format!("{}\n{name}\n", manifest.version()))
        }
        Command::Install {
            target,
            manifest,
            signature,
            archive,
        } => {
            let manifest = verified(key, places, manifest, signature)?;
            let archive = fs::read(archive).map_err(|error| {
                Failure::failed(format_args!("its archive cannot be read: {error}"))
            })?;
            let release = manifest.check(target, &archive).map_err(refused)?;
            place(places, &archive, &release.version)?;
            Ok(format!("{}\n", release.version))
        }
    }
}

fn trusted(key: Option<&str>) -> Result<TrustedKey, Failure> {
    TrustedKey::from_base64(key.ok_or(Failure::NotPublished)?).map_err(Failure::failed)
}

/// The version of the Backbone this installer put in place, `None` when
/// there is none, or [`Failure::Earlier`] when another is in the way.
fn installed(places: &Places) -> Result<Option<Version>, Failure> {
    let exists = |path: &Path| match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Failure::failed(format_args!(
            "{} cannot be read: {error}",
            path.display()
        ))),
    };
    for service in &places.earlier_services {
        if exists(service)?.is_some() {
            return Err(Failure::Earlier);
        }
    }
    let program = places.program();
    let Some(metadata) = exists(&program)? else {
        return Ok(None);
    };
    let owned = exists(&places.marker())?.is_some_and(|marker| marker.is_file());
    if !metadata.is_file() || !owned {
        return Err(Failure::Earlier);
    }
    version(&program).map(Some)
}

/// The version `program --version` reports, as `backbone X.Y.Z`.
fn version(program: &Path) -> Result<Version, Failure> {
    let unreadable = |why: &dyn fmt::Display| {
        Failure::Refused(format!(
            "Agent Backbone was left unchanged: the version of {} cannot be read: {why}.",
            program.display()
        ))
    };
    let mut attempts = 0;
    let mut child = loop {
        match std::process::Command::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            // Another thread's child can hold the program open for writing
            // until it execs; only the tests run threads.
            Err(error) if error.kind() == io::ErrorKind::ExecutableFileBusy && attempts < 50 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            spawned => break spawned.map_err(|error| unreadable(&error))?,
        }
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait().map_err(|error| unreadable(&error))? {
            Some(status) => break status,
            None if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unreadable(&"it did not answer within 10 seconds"));
            }
        }
    };
    let mut output = String::new();
    child
        .stdout
        .take()
        .expect("piped")
        .read_to_string(&mut output)
        .map_err(|error| unreadable(&error))?;
    let reported = output.trim_end();
    reported
        .strip_prefix("backbone ")
        .filter(|_| status.success())
        .and_then(|version| Version::parse(version).ok())
        .ok_or_else(|| unreadable(&format_args!("it reported {reported:?}")))
}

fn verified(
    key: Option<&str>,
    places: &Places,
    manifest: &Path,
    signature: &Path,
) -> Result<VerifiedManifest, Failure> {
    let key = trusted(key)?;
    let installed = installed(places)?;
    let unreadable = |what: &str, error: io::Error| {
        Failure::Retry(format!(
            "Agent Backbone's release {what} cannot be read: {error}."
        ))
    };
    let manifest = fs::read(manifest).map_err(|error| unreadable("manifest", error))?;
    let signature =
        fs::read_to_string(signature).map_err(|error| unreadable("signature", error))?;
    let floor = installed.unwrap_or(Version::new(0, 0, 0));
    backbone_release::verify_manifest(&manifest, &signature, &key, &floor).map_err(refused)
}

fn refused(refusal: Refusal) -> Failure {
    let why = format!("Agent Backbone's release was refused: {refusal}.");
    match refusal {
        Refusal::UnreadableSignature | Refusal::WrongKey | Refusal::BadSignature => {
            Failure::Retry(why)
        }
        _ => Failure::Refused(why),
    }
}

/// Unpacks `backbone` from `archive` beside its destination and renames it
/// into place.
///
/// The marker is written first, so an interruption leaves either no
/// program, or one this installer owns: the one it replaced, which the
/// marker already covered, or the new one.
fn place(places: &Places, archive: &[u8], version: &Version) -> Result<(), Failure> {
    fs::create_dir_all(&places.bin).map_err(Failure::failed)?;
    let mut program = tempfile::Builder::new()
        .prefix(".backbone.")
        .tempfile_in(&places.bin)
        .map_err(Failure::failed)?;
    unpack(archive, program.as_file_mut())?;
    let file = program.as_file();
    file.set_permissions(fs::Permissions::from_mode(0o755))
        .and_then(|()| file.sync_all())
        .map_err(Failure::failed)?;
    let first = !places.program().exists();
    write_marker(places, version)?;
    if let Err(error) = program.persist(places.program()) {
        if first {
            let _ = fs::remove_file(places.marker());
        }
        return Err(Failure::failed(error.error));
    }
    File::open(&places.bin)
        .and_then(|directory| directory.sync_all())
        .map_err(Failure::failed)
}

/// Copies `backbone` from `archive` to `out`.
/// The archive holds exactly `backbone` and `LICENSE`, as regular files
/// with no folder above them; anything else is refused.
fn unpack(archive: &[u8], out: &mut File) -> Result<(), Failure> {
    let refused =
        |why: String| Failure::Refused(format!("Agent Backbone's archive was refused: {why}."));
    let unreadable = |error: io::Error| refused(format!("it cannot be read: {error}"));
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let (mut program, mut license) = (false, false);
    for entry in entries.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        let path = entry.path_bytes().into_owned();
        let name = String::from_utf8_lossy(&path).into_owned();
        if !entry.header().entry_type().is_file() {
            return Err(refused(format!("{name:?} is not a regular file")));
        }
        match &path[..] {
            b"backbone" if !program => {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    let read = entry.read(&mut buffer).map_err(unreadable)?;
                    if read == 0 {
                        break;
                    }
                    out.write_all(&buffer[..read]).map_err(Failure::failed)?;
                }
                program = true;
            }
            b"LICENSE" if !license => {
                io::copy(&mut entry, &mut io::sink()).map_err(unreadable)?;
                license = true;
            }
            _ => {
                return Err(refused(format!(
                    "it holds {name:?}, which is not one of its two files"
                )));
            }
        }
    }
    if program && license {
        Ok(())
    } else {
        Err(refused("it does not hold both backbone and LICENSE".into()))
    }
}

/// Replaces the marker by a rename in its directory, recording `version`.
fn write_marker(places: &Places, version: &Version) -> Result<(), Failure> {
    let mut marker = tempfile::Builder::new()
        .prefix(".agent-interface-backbone.")
        .tempfile_in(&places.bin)
        .map_err(Failure::failed)?;
    marker
        .write_all(format!("{MARKER_HEADER}\nversion {version}\n").as_bytes())
        .and_then(|()| marker.as_file().sync_all())
        .map_err(Failure::failed)?;
    marker
        .persist(places.marker())
        .map(drop)
        .map_err(|error| Failure::failed(error.error))
}

#[cfg(test)]
mod tests {
    use std::io::Seek;

    use minisign::KeyPair;
    use sha2::{Digest, Sha256};

    use super::*;

    const TARGET: &str = "aarch64-apple-darwin";

    /// A home with a release key, and the files of releases it signs.
    struct Fixture {
        root: tempfile::TempDir,
        keys: KeyPair,
        key: String,
        places: Places,
    }

    impl Fixture {
        fn new() -> Fixture {
            let root = tempfile::tempdir().unwrap();
            let places = Places::new(&root.path().join("home"), &root.path().join("config"));
            let keys = KeyPair::generate_unencrypted_keypair().unwrap();
            let key = keys.pk.to_base64();
            Fixture {
                root,
                keys,
                key,
                places,
            }
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.root.path().join(name);
            fs::write(&path, bytes).unwrap();
            path
        }

        /// The files of a release of `version` whose archive holds
        /// `entries`, signed by the fixture's key.
        fn release(&self, version: &str, entries: &[(&str, &[u8])]) -> Command {
            let archive = archive(entries);
            let digest: String = Sha256::digest(&archive)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let manifest = format!(
                "backbone-release-manifest 1\nversion {version}\nartifact {TARGET} backbone-{version}-{TARGET}.tar.gz {digest}\n"
            );
            let signature = minisign::sign(None, &self.keys.sk, manifest.as_bytes(), None, None)
                .unwrap()
                .into_string();
            Command::Install {
                target: TARGET.into(),
                manifest: self.file("manifest", manifest.as_bytes()),
                signature: self.file("manifest.minisig", signature.as_bytes()),
                archive: self.file("archive.tar.gz", &archive),
            }
        }

        /// Installs a release of `version`.
        fn installed(&self, version: &str) {
            let program = program(version);
            let release = self.release(version, &good(&program));
            assert_eq!(self.run(&release), Ok(format!("{version}\n")));
        }

        fn run(&self, command: &Command) -> Result<String, Failure> {
            execute(command, Some(&self.key), &self.places)
        }

        /// The names in `~/.local/bin`.
        fn bin(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.places.bin)
                .map(|entries| {
                    entries
                        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            names
        }
    }

    /// A program that reports `version`, as Backbone's does.
    fn program(version: &str) -> Vec<u8> {
        format!("#!/bin/sh\necho 'backbone {version}'\n").into_bytes()
    }

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, bytes) in entries {
            let mut header = tar::Header::new_ustar();
            if let Some(link) = name.strip_prefix("link:") {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                builder.append_link(&mut header, link, "backbone").unwrap();
                continue;
            }
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            // Written as is: the builder refuses a path that leaves the archive.
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            builder.append(&header, *bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn good(program: &[u8]) -> Vec<(&str, &[u8])> {
        vec![("backbone", program), ("LICENSE", b"MIT")]
    }

    fn manifest_only(command: &Command) -> Command {
        let Command::Install {
            target,
            manifest,
            signature,
            ..
        } = command
        else {
            unreachable!()
        };
        Command::VerifyManifest {
            target: target.clone(),
            manifest: manifest.clone(),
            signature: signature.clone(),
        }
    }

    fn parse_all(arguments: &[&str]) -> Result<Command, Failure> {
        parse(arguments.iter().map(OsString::from))
    }

    #[test]
    fn commands_are_parsed() {
        assert_eq!(parse_all(&["check"]), Ok(Command::Check));
        assert_eq!(
            parse_all(&["verify-manifest", "--target", TARGET, "m", "s"]),
            Ok(Command::VerifyManifest {
                target: TARGET.into(),
                manifest: "m".into(),
                signature: "s".into(),
            })
        );
        assert_eq!(
            parse_all(&["install", "m", "s", "a", "--target", TARGET]),
            Ok(Command::Install {
                target: TARGET.into(),
                manifest: "m".into(),
                signature: "s".into(),
                archive: "a".into(),
            })
        );
        for arguments in [
            &[][..],
            &["upgrade"],
            &["check", "extra"],
            &["check", "--target", TARGET],
            &["verify-manifest", "m", "s"],
            &["verify-manifest", "--target", TARGET, "m"],
            &["install", "--target", TARGET, "m", "s"],
            &["install", "--target", "", "m", "s", "a"],
            &["install", "--target", TARGET, "--force", "m", "s", "a"],
        ] {
            let failure = parse_all(arguments).unwrap_err();
            assert_eq!(failure.status(), status::USAGE, "{arguments:?}");
        }
    }

    #[test]
    fn the_built_in_key_is_unset_or_a_key() {
        let fixture = Fixture::new();
        match RELEASE_KEY {
            None => {
                let command = fixture.release("0.2.0", &good(&program("0.2.0")));
                for command in [Command::Check, manifest_only(&command), command] {
                    let failure = execute(&command, RELEASE_KEY, &fixture.places).unwrap_err();
                    assert_eq!(failure, Failure::NotPublished);
                    assert!(failure.to_string().contains("not published yet"));
                }
                assert!(fixture.bin().is_empty());
            }
            Some(key) => assert!(TrustedKey::from_base64(key).is_ok()),
        }
    }

    #[test]
    fn a_release_is_put_in_place_by_a_rename() {
        let fixture = Fixture::new();
        let first = fixture.release("0.2.0", &good(&program("0.2.0")));
        assert_eq!(
            fixture.run(&manifest_only(&first)),
            Ok(format!("0.2.0\nbackbone-0.2.0-{TARGET}.tar.gz\n"))
        );
        assert_eq!(fixture.run(&first), Ok("0.2.0\n".into()));
        let installed = fixture.places.program();
        assert_eq!(fs::read(&installed).unwrap(), program("0.2.0"));
        assert_eq!(
            fs::metadata(&installed).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(fixture.bin(), [MARKER, "backbone"]);

        // The program open before the update keeps its bytes: the new one
        // replaced it by a rename, not by writing over it.
        let mut open = File::open(&installed).unwrap();
        fixture.installed("0.3.0");
        let mut before = Vec::new();
        open.rewind().unwrap();
        open.read_to_end(&mut before).unwrap();
        assert_eq!(before, program("0.2.0"));
        assert_eq!(fs::read(&installed).unwrap(), program("0.3.0"));
        assert_eq!(
            fs::read_to_string(fixture.places.marker()).unwrap(),
            format!("{MARKER_HEADER}\nversion 0.3.0\n")
        );
        assert_eq!(fixture.bin(), [MARKER, "backbone"]);
    }

    #[test]
    fn the_installed_version_is_the_program_s_own() {
        // As after `backbone upgrade` replaced the program in place.
        let fixture = Fixture::new();
        fixture.installed("0.2.0");
        fs::write(fixture.places.program(), program("0.4.0")).unwrap();
        assert_eq!(installed(&fixture.places), Ok(Some(Version::new(0, 4, 0))));
        let older = fixture.release("0.3.0", &good(&program("0.3.0")));
        let failure = fixture.run(&older).unwrap_err();
        assert!(
            failure.to_string().contains("older than 0.4.0"),
            "{failure}"
        );
        fixture.installed("0.5.0");

        fs::write(
            fixture.places.program(),
            b"#!/bin/sh\necho 'something else'\n",
        )
        .unwrap();
        for command in [Command::Check, manifest_only(&older)] {
            let failure = fixture.run(&command).unwrap_err();
            assert_eq!(failure.status(), status::REFUSED);
            assert!(
                failure.to_string().contains("\"something else\""),
                "{failure}"
            );
        }
    }

    #[test]
    fn an_unsafe_archive_is_refused_and_leaves_nothing_behind() {
        let fixture = Fixture::new();
        let program = program("0.2.0");
        let program = &program[..];
        for entries in [
            vec![("backbone", program)],
            vec![("backbone", program), ("LICENSE", b"MIT"), ("extra", b"")],
            vec![("bin/backbone", program), ("LICENSE", b"MIT")],
            vec![("../backbone", program), ("LICENSE", b"MIT")],
            vec![("link:LICENSE", b""), ("backbone", program)],
            vec![
                ("backbone", program),
                ("backbone", program),
                ("LICENSE", b"MIT"),
            ],
        ] {
            let failure = fixture
                .run(&fixture.release("0.2.0", &entries))
                .unwrap_err();
            assert_eq!(failure.status(), status::REFUSED, "{entries:?}: {failure}");
            assert_eq!(fixture.bin(), Vec::<String>::new(), "{entries:?}");
        }
    }

    #[test]
    fn a_bad_or_missing_signature_asks_for_a_retry() {
        let fixture = Fixture::new();
        let release = |fixture: &Fixture| {
            let Command::Install {
                manifest,
                signature,
                ..
            } = fixture.release("0.2.0", &good(&program("0.2.0")))
            else {
                unreachable!()
            };
            (manifest, signature)
        };
        let changed =
            |manifest: &Path| fs::write(manifest, b"backbone-release-manifest 1\nversion 9.0.0\n");
        let truncated = |signature: &Path| fs::write(signature, b"untrusted comment");
        let missing = |path: &Path| fs::remove_file(path);
        for (spoil, manifest_spoiled) in [
            (&changed as &dyn Fn(&Path) -> io::Result<()>, true),
            (&missing, true),
            (&truncated, false),
            (&missing, false),
        ] {
            let (manifest, signature) = release(&fixture);
            spoil(if manifest_spoiled {
                &manifest
            } else {
                &signature
            })
            .unwrap();
            let command = Command::VerifyManifest {
                target: TARGET.into(),
                manifest,
                signature,
            };
            assert_eq!(fixture.run(&command).unwrap_err().status(), status::RETRY);
        }
        // Signed by another key.
        let (manifest, signature) = release(&Fixture::new());
        let command = Command::VerifyManifest {
            target: TARGET.into(),
            manifest,
            signature,
        };
        assert_eq!(fixture.run(&command).unwrap_err().status(), status::RETRY);
        assert!(fixture.bin().is_empty());
    }

    #[test]
    fn an_older_release_a_missing_target_and_a_wrong_digest_are_refused() {
        let fixture = Fixture::new();
        fixture.installed("0.3.0");
        let older = fixture.release("0.2.0", &good(&program("0.2.0")));
        let failure = fixture.run(&manifest_only(&older)).unwrap_err();
        assert_eq!(failure.status(), status::REFUSED);
        assert!(
            failure.to_string().contains("older than 0.3.0"),
            "{failure}"
        );

        let Command::Install {
            manifest,
            signature,
            archive,
            ..
        } = fixture.release("0.4.0", &good(&program("0.4.0")))
        else {
            unreachable!()
        };
        let other_target = Command::VerifyManifest {
            target: "x86_64-unknown-linux-gnu".into(),
            manifest: manifest.clone(),
            signature: signature.clone(),
        };
        assert_eq!(
            fixture.run(&other_target).unwrap_err().status(),
            status::REFUSED
        );
        let mut bytes = fs::read(&archive).unwrap();
        bytes.push(0);
        fs::write(&archive, bytes).unwrap();
        let wrong_digest = Command::Install {
            target: TARGET.into(),
            manifest,
            signature,
            archive,
        };
        let failure = fixture.run(&wrong_digest).unwrap_err();
        assert_eq!(failure.status(), status::REFUSED);
        assert!(failure.to_string().contains("digest differs"), "{failure}");
        assert_eq!(
            fs::read(fixture.places.program()).unwrap(),
            program("0.3.0")
        );
    }

    #[test]
    fn an_earlier_installation_is_left_in_place() {
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.places.bin).unwrap();
        fs::write(fixture.places.program(), b"someone else's").unwrap();
        let release = fixture.release("0.2.0", &good(&program("0.2.0")));
        for command in [Command::Check, manifest_only(&release), release] {
            let failure = fixture.run(&command).unwrap_err();
            assert_eq!(failure, Failure::Earlier);
            assert_eq!(failure.to_string(), EARLIER);
        }
        assert_eq!(
            fs::read(fixture.places.program()).unwrap(),
            b"someone else's"
        );
        assert_eq!(fixture.bin(), ["backbone"]);

        let fixture = Fixture::new();
        for service in &fixture.places.earlier_services {
            fs::create_dir_all(service.parent().unwrap()).unwrap();
            fs::write(service, b"").unwrap();
            assert_eq!(fixture.run(&Command::Check), Err(Failure::Earlier));
            fs::remove_file(service).unwrap();
        }
        assert_eq!(fixture.run(&Command::Check), Ok(String::new()));
    }
}
