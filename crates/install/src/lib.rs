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
//! 3. `install` verifies the manifest again, checks the archive's digest
//!    and puts Backbone in place by renames. On Linux it unpacks `backbone`
//!    into `~/.local/bin`. On macOS it unpacks `Agent Backbone.app` into
//!    `~/Applications`, and makes `~/.local/bin/backbone` a symbolic link to
//!    the program inside.
//!
//! The target decides the layout: a target ending in `-apple-darwin` has
//! the bundle. The installer owns Backbone while a marker it writes beside
//! `~/.local/bin/backbone` exists and names that layout, also once
//! `backbone upgrade` has replaced the program or the bundle. Only an owned
//! program is run, with `--version`, to learn the installed version. Each
//! refusal has its own exit status, listed in [`status`].

use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{OpenOptionsExt, PermissionsExt},
    },
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
pub const USAGE: &str = "Usage: agent-interface backbone-install check --target TARGET\n       agent-interface backbone-install verify-manifest --target TARGET MANIFEST SIGNATURE\n       agent-interface backbone-install install --target TARGET MANIFEST SIGNATURE ARCHIVE\n";

/// What the installer says when an earlier Backbone is in the way.
const EARLIER: &str = "An earlier Agent Backbone was found and left in place.";

/// The marker beside `backbone` that says this installer put it there. It
/// also records the version placed, for whoever reads it; nothing does.
const MARKER: &str = ".agent-interface-backbone";
const MARKER_HEADER: &str = "agent-interface backbone 1";

/// The bundle macOS archives hold, as it is named in `~/Applications`, and
/// its program.
const BUNDLE: &str = "Agent Backbone.app";
const BUNDLE_PROGRAM: &str = "Contents/MacOS/backbone";

/// The prefix of the folder a bundle is unpacked into, in `~/Applications`.
const STAGING: &str = ".agent-backbone.";

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
    Check {
        target: String,
    },
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
        (Some("check"), []) => Ok(Command::Check {
            target: required()?,
        }),
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

/// How a target's archive is laid out, and so how Backbone is put in place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// `backbone` and `LICENSE`; `backbone` goes in `~/.local/bin`.
    Program,
    /// `Agent Backbone.app/` and `LICENSE`; the bundle goes in
    /// `~/Applications`, and `~/.local/bin/backbone` links to its program.
    Bundle,
}

impl Layout {
    fn of(target: &str) -> Layout {
        if target.ends_with("-apple-darwin") {
            Layout::Bundle
        } else {
            Layout::Program
        }
    }
}

/// Where Backbone goes, and where an earlier Backbone's login service is
/// defined.
struct Places {
    bin: PathBuf,
    applications: PathBuf,
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
            applications: home.join("Applications"),
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

    fn bundle(&self) -> PathBuf {
        self.applications.join(BUNDLE)
    }

    fn bundle_program(&self) -> PathBuf {
        self.bundle().join(BUNDLE_PROGRAM)
    }

    /// The marker's lines that name `layout`'s places, after its version.
    fn marker_lines(&self, layout: Layout) -> Vec<Vec<u8>> {
        let line =
            |key: &str, path: PathBuf| [key.as_bytes(), path.as_os_str().as_bytes()].concat();
        match layout {
            Layout::Program => Vec::new(),
            Layout::Bundle => vec![
                line("link ", self.program()),
                line("bundle ", self.bundle()),
            ],
        }
    }
}

fn execute(command: &Command, key: Option<&str>, places: &Places) -> Result<String, Failure> {
    match command {
        Command::Check { target } => {
            trusted(key)?;
            installed(places, Layout::of(target))?;
            Ok(String::new())
        }
        Command::VerifyManifest {
            target,
            manifest,
            signature,
        } => {
            let manifest = verified(key, places, Layout::of(target), manifest, signature)?;
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
            let layout = Layout::of(target);
            let manifest = verified(key, places, layout, manifest, signature)?;
            let archive = fs::read(archive).map_err(|error| {
                Failure::failed(format_args!("its archive cannot be read: {error}"))
            })?;
            let release = manifest.check(target, &archive).map_err(refused)?;
            match layout {
                Layout::Program => place(places, &archive, &release.version)?,
                Layout::Bundle => place_bundle(places, &archive, &release.version)?,
            }
            Ok(format!("{}\n", release.version))
        }
    }
}

fn trusted(key: Option<&str>) -> Result<TrustedKey, Failure> {
    TrustedKey::from_base64(key.ok_or(Failure::NotPublished)?).map_err(Failure::failed)
}

/// The version of the Backbone this installer put in place with `layout`,
/// `None` when there is none, or [`Failure::Earlier`] when another is in
/// the way.
///
/// With the bundle, an owned link that is missing or leads to a missing
/// program is no installed version: an interrupted run left it, and a
/// rerun completes it.
fn installed(places: &Places, layout: Layout) -> Result<Option<Version>, Failure> {
    for service in &places.earlier_services {
        if exists(service)?.is_some() {
            return Err(Failure::Earlier);
        }
    }
    let program = places.program();
    let found = exists(&program)?;
    let owned = marker_names(places, layout)?;
    match layout {
        Layout::Program => {
            let Some(metadata) = found else {
                return Ok(None);
            };
            if !metadata.is_file() || owned != Some(true) {
                return Err(Failure::Earlier);
            }
        }
        Layout::Bundle => {
            let bundle = exists(&places.bundle())?;
            if bundle.as_ref().is_some_and(|bundle| !bundle.is_dir()) {
                return Err(Failure::Earlier);
            }
            match owned {
                None if found.is_none() && bundle.is_none() => return Ok(None),
                Some(true) => {}
                _ => return Err(Failure::Earlier),
            }
            let Some(metadata) = found else {
                return Ok(None);
            };
            let target = metadata
                .is_symlink()
                .then(|| fs::read_link(&program))
                .transpose()
                .map_err(|error| unreadable(&program, &error))?;
            if target != Some(places.bundle_program()) {
                return Err(Failure::Earlier);
            }
            if exists(&places.bundle_program())?.is_none() {
                return Ok(None);
            }
        }
    }
    version(&program).map(Some)
}

/// Whether the marker names `layout`'s places, or `None` when there is no
/// marker. A marker that is not a regular file names none.
fn marker_names(places: &Places, layout: Layout) -> Result<Option<bool>, Failure> {
    let path = places.marker();
    let Some(metadata) = exists(&path)? else {
        return Ok(None);
    };
    if !metadata.is_file() {
        return Ok(Some(false));
    }
    let marker = fs::read(&path).map_err(|error| unreadable(&path, &error))?;
    let lines: Vec<&[u8]> = marker
        .split(|&byte| byte == b'\n')
        .filter(|line| line.starts_with(b"link ") || line.starts_with(b"bundle "))
        .collect();
    Ok(Some(lines == places.marker_lines(layout)))
}

/// The metadata of `path` itself, or `None` when nothing is there.
fn exists(path: &Path) -> Result<Option<fs::Metadata>, Failure> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unreadable(path, &error)),
    }
}

fn unreadable(path: &Path, error: &io::Error) -> Failure {
    Failure::failed(format_args!("{} cannot be read: {error}", path.display()))
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
    layout: Layout,
    manifest: &Path,
    signature: &Path,
) -> Result<VerifiedManifest, Failure> {
    let key = trusted(key)?;
    let installed = installed(places, layout)?;
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
    write_marker(places, version, Layout::Program)?;
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
                copy(&mut entry, out)?;
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

/// Unpacks `Agent Backbone.app` from `archive` into a folder beside its
/// destination, renames it into place, then links `~/.local/bin/backbone`
/// to its program by renaming a new link over it.
///
/// The marker is written first, so an interruption leaves Backbone
/// installed, or something [`installed`] owns and counts as no version,
/// which a rerun completes. A bundle already in place is renamed into the
/// folder, and removed with it.
fn place_bundle(places: &Places, archive: &[u8], version: &Version) -> Result<(), Failure> {
    for directory in [&places.bin, &places.applications] {
        fs::create_dir_all(directory).map_err(Failure::failed)?;
    }
    sweep(&places.applications)?;
    let staging = tempfile::Builder::new()
        .prefix(STAGING)
        .tempdir_in(&places.applications)
        .map_err(Failure::failed)?;
    unpack_bundle(archive, staging.path())?;
    let (link, bundle) = (places.program(), places.bundle());
    let replaced = exists(&bundle)?.is_some();
    let first = !replaced && exists(&link)?.is_none();
    write_marker(places, version, Layout::Bundle)?;
    let aside = staging.path().join("previous");
    if replaced {
        fs::rename(&bundle, &aside).map_err(Failure::failed)?;
    }
    if let Err(error) = fs::rename(staging.path().join(BUNDLE), &bundle) {
        if replaced {
            let _ = fs::rename(&aside, &bundle);
        }
        if first {
            let _ = fs::remove_file(places.marker());
        }
        return Err(Failure::failed(error));
    }
    tempfile::Builder::new()
        .prefix(".backbone.")
        .make_in(&places.bin, |path| {
            std::os::unix::fs::symlink(places.bundle_program(), path)
        })
        .and_then(|new| new.persist(&link).map_err(|error| error.error))
        .map_err(Failure::failed)?;
    for directory in [&places.applications, &places.bin] {
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(Failure::failed)?;
    }
    Ok(())
}

/// Removes the folders that interrupted runs left in `applications`. A
/// link or file with the same prefix is not this installer's, and stays.
fn sweep(applications: &Path) -> Result<(), Failure> {
    for entry in fs::read_dir(applications).map_err(Failure::failed)? {
        let entry = entry.map_err(Failure::failed)?;
        if entry.file_name().as_bytes().starts_with(STAGING.as_bytes())
            && entry.file_type().map_err(Failure::failed)?.is_dir()
        {
            // One that cannot be removed is no reason to stop.
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

/// Unpacks `Agent Backbone.app` from `archive` into `directory`.
/// The archive holds `LICENSE`, and folders and regular files under
/// `Agent Backbone.app/`, the program among them, each once, by plain
/// relative paths; anything else is refused. Files keep the archive's
/// permissions, without setuid, setgid or sticky bits. `LICENSE` is read,
/// not written.
fn unpack_bundle(archive: &[u8], directory: &Path) -> Result<(), Failure> {
    let refused =
        |why: String| Failure::Refused(format!("Agent Backbone's archive was refused: {why}."));
    let unreadable = |error: io::Error| refused(format!("it cannot be read: {error}"));
    let program_path = format!("{BUNDLE}/{BUNDLE_PROGRAM}");
    let mut entries = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let (mut seen, mut program, mut license) = (HashSet::new(), false, false);
    for entry in entries.entries().map_err(unreadable)? {
        let mut entry = entry.map_err(unreadable)?;
        let path = entry.path_bytes().into_owned();
        let name = String::from_utf8_lossy(&path).into_owned();
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(refused(format!(
                "{name:?} is not a folder or a regular file"
            )));
        }
        let relative = match path.strip_suffix(b"/") {
            Some(folder) if kind.is_dir() => folder,
            _ => &path[..],
        };
        let parts: Vec<&[u8]> = relative.split(|&byte| byte == b'/').collect();
        let plain = parts
            .iter()
            .all(|part| !part.is_empty() && *part != b"." && *part != b"..");
        let inside = parts[0] == BUNDLE.as_bytes() && (kind.is_dir() || parts.len() > 1);
        let is_license = parts == [b"LICENSE"] && kind.is_file();
        if !plain || !(inside || is_license) {
            return Err(refused(format!(
                "it holds {name:?}, which is not LICENSE or inside {BUNDLE}"
            )));
        }
        if !seen.insert(relative.to_vec()) {
            return Err(refused(format!("it holds {name:?} more than once")));
        }
        if is_license {
            io::copy(&mut entry, &mut io::sink()).map_err(unreadable)?;
            license = true;
            continue;
        }
        let out = directory.join(OsStr::from_bytes(relative));
        // A path already there differs from an earlier entry only by case,
        // or is a file where a folder is needed.
        let conflict = |error: io::Error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                refused(format!("{name:?} conflicts with another of its entries"))
            } else {
                Failure::failed(error)
            }
        };
        if kind.is_dir() {
            fs::create_dir_all(&out).map_err(conflict)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(conflict)?;
        }
        let mode = entry.header().mode().map_err(unreadable)? & 0o777;
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&out)
            .map_err(conflict)?;
        copy(&mut entry, &mut file)?;
        file.set_permissions(fs::Permissions::from_mode(mode))
            .and_then(|()| file.sync_all())
            .map_err(Failure::failed)?;
        program |= relative == program_path.as_bytes();
    }
    if program && license {
        Ok(())
    } else {
        Err(refused(format!(
            "it does not hold both LICENSE and {program_path}"
        )))
    }
}

/// Copies an archive's entry to `out`: an entry that cannot be read is
/// refused, a file that cannot be written fails.
fn copy(entry: &mut impl Read, out: &mut File) -> Result<(), Failure> {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = entry.read(&mut buffer).map_err(|error| {
            Failure::Refused(format!(
                "Agent Backbone's archive was refused: it cannot be read: {error}."
            ))
        })?;
        if read == 0 {
            return Ok(());
        }
        out.write_all(&buffer[..read]).map_err(Failure::failed)?;
    }
}

/// Replaces the marker by a rename in its directory, recording `version`
/// and the places of `layout`.
fn write_marker(places: &Places, version: &Version, layout: Layout) -> Result<(), Failure> {
    let mut marker = tempfile::Builder::new()
        .prefix(".agent-interface-backbone.")
        .tempfile_in(&places.bin)
        .map_err(Failure::failed)?;
    let mut contents = format!("{MARKER_HEADER}\nversion {version}\n").into_bytes();
    for line in places.marker_lines(layout) {
        contents.extend(line);
        contents.push(b'\n');
    }
    marker
        .write_all(&contents)
        .and_then(|()| marker.as_file().sync_all())
        .map_err(Failure::failed)?;
    marker
        .persist(places.marker())
        .map(drop)
        .map_err(|error| Failure::failed(error.error))
}

#[cfg(test)]
mod tests {
    use std::{io::Seek, os::unix::fs::symlink};

    use minisign::KeyPair;
    use sha2::{Digest, Sha256};
    use tar::EntryType;

    use super::*;

    const TARGET: &str = "aarch64-unknown-linux-gnu";
    const APPLE: &str = "aarch64-apple-darwin";

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
            self.release_for(TARGET, version, &archive(entries))
        }

        /// The files of a release of `version` for `target` with `archive`.
        fn release_for(&self, target: &str, version: &str, archive: &[u8]) -> Command {
            let digest: String = Sha256::digest(archive)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            let manifest = format!(
                "backbone-release-manifest 1\nversion {version}\nartifact {target} backbone-{version}-{target}.tar.gz {digest}\n"
            );
            let signature = minisign::sign(None, &self.keys.sk, manifest.as_bytes(), None, None)
                .unwrap()
                .into_string();
            Command::Install {
                target: target.into(),
                manifest: self.file("manifest", manifest.as_bytes()),
                signature: self.file("manifest.minisig", signature.as_bytes()),
                archive: self.file("archive.tar.gz", archive),
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

        /// The files of a release of `version` with the bundle.
        fn bundle_release(&self, version: &str) -> Command {
            let program = program(version);
            self.release_for(APPLE, version, &raw_archive(&bundle_entries(&program)))
        }

        /// Installs a release of `version` with the bundle.
        fn install_bundle(&self, version: &str) {
            let release = self.bundle_release(version);
            assert_eq!(self.run(&release), Ok(format!("{version}\n")));
        }

        /// Asserts that the bundle of `version` is in place, linked and
        /// marked, with nothing else in `~/.local/bin`.
        fn assert_bundle(&self, version: &str) {
            let places = &self.places;
            assert_eq!(fs::read(places.bundle_program()).unwrap(), program(version));
            assert_eq!(
                fs::read_link(places.program()).unwrap(),
                places.bundle_program()
            );
            assert_eq!(
                fs::read_to_string(places.marker()).unwrap(),
                format!(
                    "{MARKER_HEADER}\nversion {version}\nlink {}\nbundle {}\n",
                    places.program().display(),
                    places.bundle().display()
                )
            );
            assert_eq!(self.bin(), [MARKER, "backbone"]);
            assert_eq!(
                installed(places, Layout::Bundle),
                Ok(Some(Version::parse(version).unwrap()))
            );
        }

        /// The names in `~/.local/bin`.
        fn bin(&self) -> Vec<String> {
            names(&self.places.bin)
        }

        /// The names in `~/Applications`.
        fn applications(&self) -> Vec<String> {
            names(&self.places.applications)
        }
    }

    fn names(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(directory)
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
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

    type Entry<'a> = (EntryType, &'a str, u32, &'a [u8]);

    const PROGRAM: &str = "Agent Backbone.app/Contents/MacOS/backbone";

    /// An archive of `(type, path, mode, bytes)` entries, the paths written
    /// as is; links lead to the program.
    fn raw_archive(entries: &[Entry<'_>]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for &(kind, name, mode, bytes) in entries {
            let mut header = tar::Header::new_ustar();
            header.set_entry_type(kind);
            header.set_size(bytes.len() as u64);
            header.set_mode(mode);
            if matches!(kind, EntryType::Symlink | EntryType::Link) {
                header.set_link_name(PROGRAM).unwrap();
            }
            header.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            header.set_cksum();
            builder.append(&header, bytes).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    /// A bundle as macOS archives hold it. Its signature's folder has no
    /// entry of its own, and its signature file a setuid bit.
    fn bundle_entries(program: &[u8]) -> Vec<Entry<'_>> {
        use EntryType::{Directory, Regular};
        vec![
            (Directory, "Agent Backbone.app/", 0o755, b""),
            (Directory, "Agent Backbone.app/Contents/", 0o755, b""),
            (
                Regular,
                "Agent Backbone.app/Contents/Info.plist",
                0o644,
                b"<plist/>",
            ),
            (Directory, "Agent Backbone.app/Contents/MacOS", 0o755, b""),
            (Regular, PROGRAM, 0o755, program),
            (
                Regular,
                "Agent Backbone.app/Contents/_CodeSignature/CodeResources",
                0o4640,
                b"signed",
            ),
            (Regular, "LICENSE", 0o644, b"MIT"),
        ]
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

    fn check(target: &str) -> Command {
        Command::Check {
            target: target.into(),
        }
    }

    fn parse_all(arguments: &[&str]) -> Result<Command, Failure> {
        parse(arguments.iter().map(OsString::from))
    }

    #[test]
    fn commands_are_parsed() {
        assert_eq!(
            parse_all(&["check", "--target", TARGET]),
            Ok(Command::Check {
                target: TARGET.into()
            })
        );
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
            &["check"],
            &["check", "--target", TARGET, "extra"],
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
                for command in [check(TARGET), manifest_only(&command), command] {
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
        assert_eq!(
            installed(&fixture.places, Layout::Program),
            Ok(Some(Version::new(0, 4, 0)))
        );
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
        for command in [check(TARGET), manifest_only(&older)] {
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
        for command in [check(TARGET), manifest_only(&release), release] {
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
            assert_eq!(fixture.run(&check(TARGET)), Err(Failure::Earlier));
            fs::remove_file(service).unwrap();
        }
        assert_eq!(fixture.run(&check(TARGET)), Ok(String::new()));
    }

    #[test]
    fn a_bundle_is_put_in_place_and_linked() {
        let fixture = Fixture::new();
        assert_eq!(fixture.run(&check(APPLE)), Ok(String::new()));
        let first = fixture.bundle_release("0.2.0");
        assert_eq!(
            fixture.run(&manifest_only(&first)),
            Ok(format!("0.2.0\nbackbone-0.2.0-{APPLE}.tar.gz\n"))
        );
        assert_eq!(fixture.run(&first), Ok("0.2.0\n".into()));
        fixture.assert_bundle("0.2.0");
        assert_eq!(fixture.applications(), [BUNDLE]);
        let bundle = fixture.places.bundle();
        // LICENSE is not written.
        assert_eq!(names(&bundle), ["Contents"]);
        assert_eq!(
            names(&bundle.join("Contents")),
            ["Info.plist", "MacOS", "_CodeSignature"]
        );
        let mode = |path: &str| {
            fs::metadata(bundle.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777
        };
        assert_eq!(mode("Contents/MacOS/backbone"), 0o755);
        assert_eq!(mode("Contents/Info.plist"), 0o644);
        assert_eq!(mode("Contents/_CodeSignature/CodeResources"), 0o640);

        // A file open in the bundle before the update keeps its bytes: the
        // new bundle replaced the old one by a rename, not by writing in it.
        let mut open = File::open(fixture.places.bundle_program()).unwrap();
        fixture.install_bundle("0.3.0");
        let mut before = Vec::new();
        open.read_to_end(&mut before).unwrap();
        assert_eq!(before, program("0.2.0"));
        fixture.assert_bundle("0.3.0");
        assert_eq!(fixture.applications(), [BUNDLE]);
    }

    #[test]
    fn the_bundle_s_version_is_read_through_the_link() {
        // As after `backbone upgrade` replaced the whole bundle in place.
        let fixture = Fixture::new();
        fixture.install_bundle("0.2.0");
        let program_path = fixture.places.bundle_program();
        fs::remove_dir_all(fixture.places.bundle()).unwrap();
        fs::create_dir_all(program_path.parent().unwrap()).unwrap();
        fs::write(&program_path, program("0.4.0")).unwrap();
        fs::set_permissions(&program_path, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            installed(&fixture.places, Layout::Bundle),
            Ok(Some(Version::new(0, 4, 0)))
        );
        let failure = fixture.run(&fixture.bundle_release("0.3.0")).unwrap_err();
        assert!(
            failure.to_string().contains("older than 0.4.0"),
            "{failure}"
        );
        fixture.install_bundle("0.5.0");
        fixture.assert_bundle("0.5.0");
    }

    #[test]
    fn an_unsafe_bundle_is_refused_and_leaves_nothing_behind() {
        use EntryType::{Directory, Regular};
        let fixture = Fixture::new();
        let program = program("0.2.0");
        let good = bundle_entries(&program);
        let with = |entry: Entry<'static>| {
            let mut entries = good.clone();
            entries.push(entry);
            entries
        };
        let without = |path: &str| {
            let mut entries = good.clone();
            entries.retain(|entry| entry.1 != path);
            entries
        };
        let mut program_folder = without(PROGRAM);
        program_folder.push((
            Directory,
            "Agent Backbone.app/Contents/MacOS/backbone/",
            0o755,
            b"",
        ));
        for entries in [
            with((
                EntryType::Symlink,
                "Agent Backbone.app/Contents/link",
                0o755,
                b"",
            )),
            with((
                EntryType::Link,
                "Agent Backbone.app/Contents/hard",
                0o755,
                b"",
            )),
            with((
                EntryType::Fifo,
                "Agent Backbone.app/Contents/fifo",
                0o644,
                b"",
            )),
            with((Regular, "/Agent Backbone.app/Contents/extra", 0o644, b"")),
            with((Regular, "Agent Backbone.app/../extra", 0o644, b"")),
            with((Regular, "Agent Backbone.app/./extra", 0o644, b"")),
            with((Regular, "Agent Backbone.app//extra", 0o644, b"")),
            with((Regular, "Agent Backbone.app/extra/", 0o644, b"")),
            with((Regular, "extra", 0o644, b"")),
            with((Regular, "._LICENSE", 0o644, b"")),
            with((Directory, "Contents/", 0o755, b"")),
            with((
                Regular,
                "Agent Backbone.app/Contents/Info.plist",
                0o644,
                b"",
            )),
            with((Regular, "LICENSE", 0o644, b"MIT")),
            with((
                Regular,
                "Agent Backbone.app/Contents/Info.plist/extra",
                0o644,
                b"",
            )),
            vec![
                (Regular, "Agent Backbone.app", 0o755, &program),
                (Regular, "LICENSE", 0o644, b"MIT"),
            ],
            vec![
                (Regular, "backbone", 0o755, &program),
                (Regular, "LICENSE", 0o644, b"MIT"),
            ],
            without(PROGRAM),
            without("LICENSE"),
            program_folder,
        ] {
            let release = fixture.release_for(APPLE, "0.2.0", &raw_archive(&entries));
            let failure = fixture.run(&release).unwrap_err();
            assert_eq!(failure.status(), status::REFUSED, "{entries:?}: {failure}");
            assert_eq!(fixture.bin(), Vec::<String>::new(), "{entries:?}");
            assert_eq!(fixture.applications(), Vec::<String>::new(), "{entries:?}");
        }
    }

    #[test]
    fn an_interrupted_bundle_install_is_completed_by_a_rerun() {
        // Marked, before the first bundle was renamed into place.
        let fixture = Fixture::new();
        fs::create_dir_all(&fixture.places.bin).unwrap();
        write_marker(&fixture.places, &Version::new(0, 2, 0), Layout::Bundle).unwrap();
        assert_eq!(installed(&fixture.places, Layout::Bundle), Ok(None));
        fixture.install_bundle("0.2.0");
        fixture.assert_bundle("0.2.0");

        // Between the renames that replace a bundle: the old one is aside
        // in the unpacking folder, and the link leads nowhere.
        let fixture = Fixture::new();
        fixture.install_bundle("0.2.0");
        let staging = fixture.places.applications.join(".agent-backbone.left");
        fs::create_dir(&staging).unwrap();
        fs::rename(fixture.places.bundle(), staging.join("previous")).unwrap();
        assert_eq!(installed(&fixture.places, Layout::Bundle), Ok(None));
        fixture.install_bundle("0.3.0");
        fixture.assert_bundle("0.3.0");
        assert_eq!(fixture.applications(), [BUNDLE]);

        // After the bundle was put in place, before the link was.
        let fixture = Fixture::new();
        fixture.install_bundle("0.2.0");
        fs::remove_file(fixture.places.program()).unwrap();
        assert_eq!(installed(&fixture.places, Layout::Bundle), Ok(None));
        fixture.install_bundle("0.2.0");
        fixture.assert_bundle("0.2.0");
    }

    #[test]
    fn only_the_installer_s_own_folders_are_swept() {
        let fixture = Fixture::new();
        let applications = &fixture.places.applications;
        let elsewhere = fixture.root.path().join("elsewhere");
        fs::create_dir_all(elsewhere.join("kept")).unwrap();
        fs::create_dir_all(applications.join("Other.app/Contents")).unwrap();
        fs::create_dir_all(applications.join(".agent-backbone")).unwrap();
        fs::create_dir_all(applications.join(".agent-backbone.left/previous")).unwrap();
        fs::write(applications.join(".agent-backbone.file"), b"").unwrap();
        symlink(&elsewhere, applications.join(".agent-backbone.link")).unwrap();

        // Nothing is swept while an earlier Backbone is in the way.
        fs::create_dir_all(&fixture.places.bin).unwrap();
        fs::write(fixture.places.program(), b"someone else's").unwrap();
        let before = fixture.applications();
        let release = fixture.bundle_release("0.2.0");
        assert_eq!(fixture.run(&release), Err(Failure::Earlier));
        assert_eq!(fixture.applications(), before);

        fs::remove_file(fixture.places.program()).unwrap();
        fixture.install_bundle("0.2.0");
        assert_eq!(
            fixture.applications(),
            [
                ".agent-backbone",
                ".agent-backbone.file",
                ".agent-backbone.link",
                BUNDLE,
                "Other.app"
            ]
        );
        assert_eq!(names(&elsewhere), ["kept"]);
        assert_eq!(names(&applications.join("Other.app")), ["Contents"]);
    }

    #[test]
    fn an_earlier_bundle_or_link_is_left_in_place() {
        type Setup = fn(&Fixture);
        fn marked(fixture: &Fixture, layout: Layout) {
            fs::create_dir_all(&fixture.places.bin).unwrap();
            write_marker(&fixture.places, &Version::new(0, 2, 0), layout).unwrap();
        }
        fn bundle(fixture: &Fixture) {
            fs::create_dir_all(fixture.places.bundle_program().parent().unwrap()).unwrap();
            fs::write(fixture.places.bundle_program(), program("0.2.0")).unwrap();
        }
        fn link(fixture: &Fixture, to: &Path) {
            fs::create_dir_all(&fixture.places.bin).unwrap();
            symlink(to, fixture.places.program()).unwrap();
        }
        let cases: [(&str, Setup); 11] = [
            ("a program, no marker", |fixture| {
                link(fixture, Path::new("/bin/sh"));
            }),
            ("a bundle, no marker", bundle),
            ("a link to the bundle, no marker", |fixture| {
                bundle(fixture);
                link(fixture, &fixture.places.bundle_program());
            }),
            ("a program", |fixture| {
                marked(fixture, Layout::Bundle);
                fs::write(fixture.places.program(), program("0.2.0")).unwrap();
            }),
            ("a folder", |fixture| {
                marked(fixture, Layout::Bundle);
                fs::create_dir(fixture.places.program()).unwrap();
            }),
            ("a link elsewhere", |fixture| {
                marked(fixture, Layout::Bundle);
                bundle(fixture);
                link(fixture, &fixture.root.path().join("backbone"));
            }),
            ("a relative link to the bundle", |fixture| {
                marked(fixture, Layout::Bundle);
                bundle(fixture);
                link(
                    fixture,
                    Path::new("../../Applications/Agent Backbone.app/Contents/MacOS/backbone"),
                );
            }),
            ("a bundle that is a file", |fixture| {
                marked(fixture, Layout::Bundle);
                fs::create_dir_all(&fixture.places.applications).unwrap();
                fs::write(fixture.places.bundle(), b"").unwrap();
            }),
            ("a bundle that is a link", |fixture| {
                marked(fixture, Layout::Bundle);
                fs::create_dir_all(&fixture.places.applications).unwrap();
                symlink(fixture.root.path(), fixture.places.bundle()).unwrap();
            }),
            ("the program layout's marker", |fixture| {
                marked(fixture, Layout::Program);
            }),
            ("a marker naming other places", |fixture| {
                fs::create_dir_all(&fixture.places.bin).unwrap();
                let marker = format!(
                    "{MARKER_HEADER}\nversion 0.2.0\nlink /elsewhere/backbone\nbundle /elsewhere/{BUNDLE}\n"
                );
                fs::write(fixture.places.marker(), marker).unwrap();
            }),
        ];
        for (case, setup) in cases {
            let fixture = Fixture::new();
            setup(&fixture);
            let before = (fixture.bin(), fixture.applications());
            let release = fixture.bundle_release("0.2.0");
            for command in [check(APPLE), manifest_only(&release), release] {
                assert_eq!(fixture.run(&command), Err(Failure::Earlier), "{case}");
            }
            assert_eq!((fixture.bin(), fixture.applications()), before, "{case}");
        }

        // The bundle's marker is not the program layout's.
        let fixture = Fixture::new();
        marked(&fixture, Layout::Bundle);
        fs::write(fixture.places.program(), program("0.2.0")).unwrap();
        assert_eq!(fixture.run(&check(TARGET)), Err(Failure::Earlier));

        // Owned, with no installed version; a rerun completes each.
        let cases: [(&str, Setup); 3] = [
            ("a marker", |_| {}),
            ("a marker and a bundle", bundle),
            ("a marker and a link to a missing program", |fixture| {
                link(fixture, &fixture.places.bundle_program());
            }),
        ];
        for (case, setup) in cases {
            let fixture = Fixture::new();
            marked(&fixture, Layout::Bundle);
            setup(&fixture);
            assert_eq!(fixture.run(&check(APPLE)), Ok(String::new()), "{case}");
            fixture.install_bundle("0.2.0");
            fixture.assert_bundle("0.2.0");
        }
    }
}
