//! Verifying a release of Backbone before it is installed.
//!
//! A release is published with a manifest, which names its version and,
//! for each target, the artifact's file name and SHA-256 digest, and a
//! [minisign] signature of the manifest made with the release key, which
//! is kept offline. A release is checked in two steps, in this order:
//!
//! 1. [`verify_manifest`] checks that the manifest is signed by the
//!    trusted key, and is unchanged, and that its version is not older
//!    than the one installed (the same version is accepted, so a damaged
//!    install can be repaired). The [`VerifiedManifest`] it gives names
//!    the version and each target's artifact, before any artifact is
//!    downloaded.
//! 2. [`VerifiedManifest::check`] checks that the artifact's digest is the
//!    one the manifest names for the target.
//!
//! [`verify`] makes both steps at once, for a caller that has the whole
//! release at hand. Nothing in the manifest is read before its signature
//! holds, and each refusal is its own [`Refusal`]. `backbone upgrade` and
//! Agent Interface's installer both verify a release this way, so this
//! crate depends on nothing else in the workspace.
//!
//! # The manifest
//!
//! Plain UTF-8 text, one entry to a line, fields split by one space:
//!
//! ```text
//! backbone-release-manifest 1
//! version 0.2.0
//! artifact aarch64-apple-darwin backbone-0.2.0-aarch64-apple-darwin.tar.gz <digest>
//! artifact x86_64-unknown-linux-gnu backbone-0.2.0-x86_64-unknown-linux-gnu.tar.gz <digest>
//! ```
//!
//! The first line names the format and its version. Exactly one `version`
//! line follows, with a semantic version, then one `artifact` line per
//! target: the target, the file name and the SHA-256 digest as 64
//! lowercase hexadecimal digits. Neither the target nor the name holds a
//! space, and a target is named once. The text may end with one line end.
//! Anything else makes the manifest malformed.
//!
//! [minisign]: https://jedisct1.github.io/minisign/

mod manifest;

use std::fmt;

use minisign_verify::{PublicKey, Signature};
use semver::Version;
use sha2::{Digest, Sha256};

/// The release key's public half, which a release must be signed with.
#[derive(Clone, Debug)]
pub struct TrustedKey(PublicKey);

impl TrustedKey {
    /// The key written as `text`, the base64 line of a minisign public key
    /// (the second line of its `.pub` file).
    pub fn from_base64(text: &str) -> Result<TrustedKey, InvalidKey> {
        PublicKey::from_base64(text.trim())
            .map(TrustedKey)
            .map_err(|_| InvalidKey)
    }
}

/// The text given for the trusted key is not a minisign public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidKey;

impl fmt::Display for InvalidKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the trusted key is not a minisign public key")
    }
}

impl std::error::Error for InvalidKey {}

/// A release as downloaded, to verify before it is installed.
#[derive(Clone, Copy, Debug)]
pub struct Release<'a> {
    /// The manifest, as published.
    pub manifest: &'a [u8],
    /// The text of the manifest's minisign signature file.
    pub signature: &'a str,
    /// The target the artifact is for, such as `aarch64-apple-darwin`.
    pub target: &'a str,
    /// The artifact.
    pub artifact: &'a [u8],
}

/// A release that may be installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    /// Its version.
    pub version: Version,
    /// The artifact's file name, as the manifest gives it.
    pub name: String,
}

/// Why a release may not be installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The signature file could not be read.
    UnreadableSignature,
    /// The signature names another key than the trusted one, by the key
    /// ID it carries, which nothing authenticates: either way it is not
    /// the trusted key's.
    WrongKey,
    /// The signature does not hold for the manifest by the trusted key,
    /// or is of a kind not taken (a legacy signature, of the unhashed
    /// manifest): the manifest or its signature was changed, or was not
    /// made as releases are.
    BadSignature,
    /// The manifest is signed but cannot be read, so it says nothing.
    Malformed(String),
    /// The release is older than the version installed. It is returned
    /// before the artifact's digest is checked, so it proves the
    /// manifest's signature only, never the artifact. An installer that
    /// offers a deliberate rollback verifies the manifest again with a
    /// lower installed version, such as 0.0.0, and installs only on
    /// [`Verified`], never on this refusal.
    Older {
        /// The version installed.
        installed: Version,
        /// The release's version.
        offered: Version,
    },
    /// The release has no artifact for this target.
    NoArtifact {
        /// The target asked for.
        target: String,
    },
    /// The artifact's SHA-256 digest is not the one the manifest names.
    WrongDigest {
        /// The artifact's file name, as the manifest gives it.
        name: String,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::UnreadableSignature => f.write_str("the release's signature cannot be read"),
            Refusal::WrongKey => {
                f.write_str("the release's signature names another key than the trusted one")
            }
            Refusal::BadSignature => f.write_str(
                "the release's signature does not hold for its manifest by the trusted key, \
                 or is not of the kind releases are signed with",
            ),
            Refusal::Malformed(why) => write!(f, "the release's manifest cannot be read: {why}"),
            Refusal::Older { installed, offered } => write!(
                f,
                "the release is version {offered}, older than {installed}, which is installed"
            ),
            Refusal::NoArtifact { target } => write!(f, "the release has nothing for {target}"),
            Refusal::WrongDigest { name } => write!(
                f,
                "{name} is not the artifact the release names: its digest differs"
            ),
        }
    }
}

impl std::error::Error for Refusal {}

/// A manifest whose signature holds by the trusted key and whose version
/// is not older than the one installed, as [`verify_manifest`] gives it.
///
/// It says what the release offers, before its artifact is downloaded;
/// only [`VerifiedManifest::check`] lets an artifact be installed.
#[derive(Clone, Debug)]
pub struct VerifiedManifest(manifest::Manifest);

impl VerifiedManifest {
    /// The release's version.
    pub fn version(&self) -> &Version {
        &self.0.version
    }

    /// The file name of the artifact for `target`, as the manifest gives
    /// it, or [`Refusal::NoArtifact`] when the release has none.
    pub fn artifact(&self, target: &str) -> Result<&str, Refusal> {
        self.entry(target).map(|artifact| artifact.name.as_str())
    }

    /// Whether `artifact` is the one the manifest names for `target`, by
    /// its SHA-256 digest. Only [`Verified`] lets it be installed.
    pub fn check(&self, target: &str, artifact: &[u8]) -> Result<Verified, Refusal> {
        let entry = self.entry(target)?;
        if Sha256::digest(artifact)[..] != entry.sha256 {
            return Err(Refusal::WrongDigest {
                name: entry.name.clone(),
            });
        }
        Ok(Verified {
            version: self.0.version.clone(),
            name: entry.name.clone(),
        })
    }

    fn entry(&self, target: &str) -> Result<&manifest::Artifact, Refusal> {
        self.0.artifact(target).ok_or_else(|| Refusal::NoArtifact {
            target: target.to_owned(),
        })
    }
}

/// Whether the release `manifest` names, with its `signature`, may replace
/// `installed`: signed by `trusted`, and not older. With nothing
/// installed, the caller passes 0.0.0.
///
/// No artifact is needed: the [`VerifiedManifest`] gives the version and
/// the artifact's file name, to download it by, and checks it once it is
/// downloaded.
pub fn verify_manifest(
    manifest: &[u8],
    signature: &str,
    trusted: &TrustedKey,
    installed: &Version,
) -> Result<VerifiedManifest, Refusal> {
    let signature = Signature::decode(signature).map_err(|_| Refusal::UnreadableSignature)?;
    // Only a signature of the manifest's hash, as current minisign makes,
    // is taken, never the legacy kind.
    trusted
        .0
        .verify(manifest, &signature, false)
        .map_err(|error| match error {
            minisign_verify::Error::UnexpectedKeyId => Refusal::WrongKey,
            _ => Refusal::BadSignature,
        })?;
    let manifest = manifest::parse(manifest).map_err(Refusal::Malformed)?;
    if manifest.version.cmp_precedence(installed).is_lt() {
        return Err(Refusal::Older {
            installed: installed.clone(),
            offered: manifest.version,
        });
    }
    Ok(VerifiedManifest(manifest))
}

/// Whether `release` may replace `installed`: signed by `trusted`, not
/// older, and with the artifact its manifest names. It is
/// [`verify_manifest`] and then [`VerifiedManifest::check`].
///
/// Only [`Verified`] lets the artifact be installed. [`Refusal::Older`]
/// comes before the artifact is looked at, so it says nothing of the
/// artifact: a deliberate rollback is verified again against a lower
/// installed version, such as 0.0.0, and installed only if that passes.
pub fn verify(
    release: &Release<'_>,
    trusted: &TrustedKey,
    installed: &Version,
) -> Result<Verified, Refusal> {
    verify_manifest(release.manifest, release.signature, trusted, installed)?
        .check(release.target, release.artifact)
}
