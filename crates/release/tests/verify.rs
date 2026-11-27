//! Verifying a release, signed here with a key the test makes.

use backbone_release::{Refusal, Release, TrustedKey, Verified, verify, verify_manifest};
use minisign::KeyPair;
use semver::Version;
use sha2::{Digest, Sha256};

const TARGET: &str = "aarch64-apple-darwin";
const NAME: &str = "backbone-0.2.0-aarch64-apple-darwin.tar.gz";
const ARTIFACT: &[u8] = b"the release's archive";

/// A release key, and the manifest and artifact it signs.
struct Signer {
    keys: KeyPair,
}

impl Signer {
    fn new() -> Signer {
        Signer {
            keys: KeyPair::generate_unencrypted_keypair().unwrap(),
        }
    }

    fn trusted(&self) -> TrustedKey {
        TrustedKey::from_base64(&self.keys.pk.to_base64()).unwrap()
    }

    /// The text of a signature of `manifest`.
    fn sign(&self, manifest: &[u8]) -> String {
        minisign::sign(None, &self.keys.sk, manifest, None, None)
            .unwrap()
            .into_string()
    }
}

/// A manifest of `version` naming `digest` for the artifact.
fn manifest(version: &str, digest: &[u8]) -> Vec<u8> {
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("backbone-release-manifest 1\nversion {version}\nartifact {TARGET} {NAME} {hex}\n")
        .into_bytes()
}

fn installed() -> Version {
    Version::new(0, 2, 0)
}

fn release<'a>(manifest: &'a [u8], signature: &'a str, artifact: &'a [u8]) -> Release<'a> {
    Release {
        manifest,
        signature,
        target: TARGET,
        artifact,
    }
}

#[test]
fn a_good_artifact_passes() {
    let signer = Signer::new();
    for version in ["0.2.0", "0.3.0"] {
        let manifest = manifest(version, &Sha256::digest(ARTIFACT));
        let signature = signer.sign(&manifest);

        let verified = verify(
            &release(&manifest, &signature, ARTIFACT),
            &signer.trusted(),
            &installed(),
        );

        assert_eq!(
            verified,
            Ok(Verified {
                version: Version::parse(version).unwrap(),
                name: NAME.into(),
            })
        );
    }
}

#[test]
fn a_changed_byte_of_the_manifest_breaks_its_signature() {
    let signer = Signer::new();
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);
    let mut changed = manifest.clone();
    // The version 0.3.0 becomes 0.4.0.
    let at = changed.iter().position(|byte| *byte == b'3').unwrap();
    changed[at] = b'4';

    let refused = verify(
        &release(&changed, &signature, ARTIFACT),
        &signer.trusted(),
        &installed(),
    );

    assert_eq!(refused, Err(Refusal::BadSignature));
}

#[test]
fn a_release_signed_by_another_key_is_refused() {
    let (signer, other) = (Signer::new(), Signer::new());
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let signature = other.sign(&manifest);

    let refused = verify(
        &release(&manifest, &signature, ARTIFACT),
        &signer.trusted(),
        &installed(),
    );

    assert_eq!(refused, Err(Refusal::WrongKey));
}

#[test]
fn an_artifact_whose_digest_differs_is_refused() {
    let signer = Signer::new();
    let wrong = manifest("0.3.0", &Sha256::digest(b"another archive"));
    let right = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let mut changed = ARTIFACT.to_vec();
    changed[0] ^= 1;
    let refused = Refusal::WrongDigest { name: NAME.into() };

    for (manifest, artifact) in [(&wrong, ARTIFACT), (&right, &changed[..])] {
        let signature = signer.sign(manifest);
        let verified = verify(
            &release(manifest, &signature, artifact),
            &signer.trusted(),
            &installed(),
        );
        assert_eq!(verified, Err(refused.clone()));
    }
}

#[test]
fn an_older_version_is_refused() {
    let signer = Signer::new();
    let manifest = manifest("0.1.9", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);

    let refused = verify(
        &release(&manifest, &signature, ARTIFACT),
        &signer.trusted(),
        &installed(),
    );

    assert_eq!(
        refused,
        Err(Refusal::Older {
            installed: installed(),
            offered: Version::new(0, 1, 9),
        })
    );
}

#[test]
fn build_metadata_does_not_make_the_installed_version_s_release_older() {
    let signer = Signer::new();
    let manifest = manifest("0.2.0", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);
    let installed = Version::parse("0.2.0+local").unwrap();

    let verified = verify(
        &release(&manifest, &signature, ARTIFACT),
        &signer.trusted(),
        &installed,
    );

    assert!(verified.is_ok(), "{verified:?}");
}

#[test]
fn an_older_release_is_installed_only_once_verified_against_a_lower_version() {
    let signer = Signer::new();
    let manifest = manifest("0.1.9", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);
    let mut changed = ARTIFACT.to_vec();
    changed[0] ^= 1;
    let older = Err(Refusal::Older {
        installed: installed(),
        offered: Version::new(0, 1, 9),
    });
    let lowest = Version::new(0, 0, 0);

    let good = release(&manifest, &signature, ARTIFACT);
    assert_eq!(verify(&good, &signer.trusted(), &installed()), older);
    assert_eq!(
        verify(&good, &signer.trusted(), &lowest),
        Ok(Verified {
            version: Version::new(0, 1, 9),
            name: NAME.into(),
        })
    );

    // Older says nothing of the artifact: only the second check finds it
    // changed.
    let tampered = release(&manifest, &signature, &changed);
    assert_eq!(verify(&tampered, &signer.trusted(), &installed()), older);
    assert_eq!(
        verify(&tampered, &signer.trusted(), &lowest),
        Err(Refusal::WrongDigest { name: NAME.into() })
    );
}

#[test]
fn nothing_unsigned_is_read() {
    let signer = Signer::new();
    let manifest = b"not a manifest".to_vec();
    let signature = signer.sign(&manifest);
    let trusted = signer.trusted();

    let unreadable = verify(
        &release(&manifest, "not a signature", ARTIFACT),
        &trusted,
        &installed(),
    );
    assert_eq!(unreadable, Err(Refusal::UnreadableSignature));
    let malformed = verify(
        &release(&manifest, &signature, ARTIFACT),
        &trusted,
        &installed(),
    );
    assert!(
        matches!(malformed, Err(Refusal::Malformed(_))),
        "{malformed:?}"
    );
}

#[test]
fn a_release_without_this_target_is_refused() {
    let signer = Signer::new();
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);
    let mut other = release(&manifest, &signature, ARTIFACT);
    other.target = "x86_64-unknown-linux-gnu";

    let refused = verify(&other, &signer.trusted(), &installed());

    assert_eq!(
        refused,
        Err(Refusal::NoArtifact {
            target: "x86_64-unknown-linux-gnu".into(),
        })
    );
}

#[test]
fn a_manifest_verified_alone_gives_its_version_and_the_target_s_file_name() {
    let signer = Signer::new();
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);

    let verified = verify_manifest(&manifest, &signature, &signer.trusted(), &installed()).unwrap();

    assert_eq!(verified.version(), &Version::new(0, 3, 0));
    assert_eq!(verified.artifact(TARGET), Ok(NAME));
    assert_eq!(
        verified.check(TARGET, ARTIFACT),
        Ok(Verified {
            version: Version::new(0, 3, 0),
            name: NAME.into(),
        })
    );
}

#[test]
fn a_manifest_is_refused_alone_before_any_artifact_is_given() {
    let (signer, other) = (Signer::new(), Signer::new());
    let older = manifest("0.1.9", &Sha256::digest(ARTIFACT));
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let mut changed = manifest.clone();
    let at = changed.iter().position(|byte| *byte == b'3').unwrap();
    changed[at] = b'4';
    let trusted = signer.trusted();

    let tampered = verify_manifest(&changed, &signer.sign(&manifest), &trusted, &installed());
    let wrong_key = verify_manifest(&manifest, &other.sign(&manifest), &trusted, &installed());
    let too_old = verify_manifest(&older, &signer.sign(&older), &trusted, &installed());

    assert_eq!(tampered.err(), Some(Refusal::BadSignature));
    assert_eq!(wrong_key.err(), Some(Refusal::WrongKey));
    assert_eq!(
        too_old.err(),
        Some(Refusal::Older {
            installed: installed(),
            offered: Version::new(0, 1, 9),
        })
    );
}

#[test]
fn a_verified_manifest_refuses_a_missing_target_and_a_wrong_digest() {
    let signer = Signer::new();
    let manifest = manifest("0.3.0", &Sha256::digest(ARTIFACT));
    let signature = signer.sign(&manifest);
    let verified = verify_manifest(&manifest, &signature, &signer.trusted(), &installed()).unwrap();
    let missing = Refusal::NoArtifact {
        target: "x86_64-unknown-linux-gnu".into(),
    };

    assert_eq!(
        verified.artifact("x86_64-unknown-linux-gnu"),
        Err(missing.clone())
    );
    assert_eq!(
        verified.check("x86_64-unknown-linux-gnu", ARTIFACT),
        Err(missing)
    );
    assert_eq!(
        verified.check(TARGET, b"another archive"),
        Err(Refusal::WrongDigest { name: NAME.into() })
    );
}
