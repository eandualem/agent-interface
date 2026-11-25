//! Reading the release manifest, in the format the crate's documentation
//! sets out.

use semver::Version;

/// The first line of every manifest of this format.
const HEADER: &str = "backbone-release-manifest 1";

/// A manifest, as read once its signature held.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) version: Version,
    pub(crate) artifacts: Vec<Artifact>,
}

/// One target's artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Artifact {
    pub(crate) target: String,
    pub(crate) name: String,
    pub(crate) sha256: [u8; 32],
}

impl Manifest {
    /// The artifact for `target`, if the release has one.
    pub(crate) fn artifact(&self, target: &str) -> Option<&Artifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.target == target)
    }
}

/// Reads `bytes` as a manifest, or says why it is not one.
pub(crate) fn parse(bytes: &[u8]) -> Result<Manifest, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "it is not UTF-8 text".to_owned())?;
    let mut lines = text.strip_suffix('\n').unwrap_or(text).split('\n');
    if lines.next() != Some(HEADER) {
        return Err(format!("its first line is not `{HEADER}`"));
    }
    let version = match lines.next().and_then(|line| line.strip_prefix("version ")) {
        Some(version) => {
            Version::parse(version).map_err(|error| format!("its version {version:?}: {error}"))?
        }
        None => return Err("its second line does not give the version".to_owned()),
    };
    let mut artifacts: Vec<Artifact> = Vec::new();
    for line in lines {
        let artifact = artifact(line)?;
        if artifacts
            .iter()
            .any(|known| known.target == artifact.target)
        {
            return Err(format!("it names target {} twice", artifact.target));
        }
        artifacts.push(artifact);
    }
    Ok(Manifest { version, artifacts })
}

fn artifact(line: &str) -> Result<Artifact, String> {
    let unreadable = || format!("the line {line:?} is not an artifact");
    let mut fields = line.split(' ');
    if fields.next() != Some("artifact") {
        return Err(unreadable());
    }
    let (Some(target), Some(name), Some(digest), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(unreadable());
    };
    if target.is_empty() || name.is_empty() {
        return Err(unreadable());
    }
    let sha256 = hex_digest(digest).ok_or_else(unreadable)?;
    Ok(Artifact {
        target: target.to_owned(),
        name: name.to_owned(),
        sha256,
    })
}

/// The 32 bytes written as 64 lowercase hexadecimal digits in `text`.
fn hex_digest(text: &str) -> Option<[u8; 32]> {
    let digits = text.as_bytes();
    if digits.len() != 64 {
        return None;
    }
    let mut bytes = [0; 32];
    for (byte, pair) in bytes.iter_mut().zip(digits.chunks(2)) {
        *byte = digit(pair[0])? << 4 | digit(pair[1])?;
    }
    Some(bytes)
}

fn digit(character: u8) -> Option<u8> {
    match character {
        b'0'..=b'9' => Some(character - b'0'),
        b'a'..=b'f' => Some(character - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn a_manifest_is_read_with_each_target_s_artifact() {
        let text = format!(
            "{HEADER}\nversion 0.2.0\nartifact aarch64-apple-darwin backbone.tar.gz {DIGEST}\n"
        );
        let manifest = parse(text.as_bytes()).unwrap();
        assert_eq!(manifest.version, Version::new(0, 2, 0));
        let artifact = manifest.artifact("aarch64-apple-darwin").unwrap();
        assert_eq!(artifact.name, "backbone.tar.gz");
        assert_eq!(artifact.sha256[..2], [0x00, 0x11]);
        assert!(manifest.artifact("x86_64-unknown-linux-gnu").is_none());
    }

    #[test]
    fn anything_but_the_format_is_malformed() {
        let line = format!("artifact aarch64-apple-darwin backbone.tar.gz {DIGEST}");
        for text in [
            format!("version 0.2.0\n{line}"),
            format!("{HEADER}\nversion two\n{line}"),
            format!("{HEADER}\n{line}"),
            format!("{HEADER}\nversion 0.2.0\n{line} extra"),
            format!(
                "{HEADER}\nversion 0.2.0\nartifact aarch64-apple-darwin backbone.tar.gz {}",
                DIGEST.to_uppercase()
            ),
            format!("{HEADER}\nversion 0.2.0\n{line}\n{line}"),
            format!("{HEADER}\nversion 0.2.0\n{line}\n\n"),
        ] {
            assert!(parse(text.as_bytes()).is_err(), "{text:?}");
        }
    }
}
