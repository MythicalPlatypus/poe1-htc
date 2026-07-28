use std::fmt;

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};

const FINGERPRINT_PREFIX: &str = "repoe-bundle-v1:sha256:";
const BUNDLE_HASH_DOMAIN: &[u8] = b"poe1-htc/repoe-source-bundle/v1";

pub(crate) const REPOE_BUNDLE_FILENAMES: [&str; 5] = [
    "mods.json",
    "base_items.json",
    "crafting_bench_options.json",
    "essences.json",
    "fossils.json",
];

/// Stable SHA-256 identity for the exact bytes of a loaded RePoE source bundle.
///
/// Values are intentionally opaque. Their textual representation is versioned
/// so a future change to the bundle framing can coexist with existing saved
/// results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DataFingerprint(String);

impl DataFingerprint {
    /// Hash an arbitrary byte sequence into the same stable fingerprint form.
    ///
    /// This is useful when an in-memory caller has one canonical source blob.
    /// Directory loads use collision-safe framing across the five known RePoE
    /// files instead.
    pub fn sha256_of_bytes(bytes: impl AsRef<[u8]>) -> Self {
        let digest = Sha256::digest(bytes.as_ref());
        Self::from_digest(&digest)
    }

    /// Return the versioned, lowercase fingerprint text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn from_digest(digest: &[u8]) -> Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";

        let mut value = String::with_capacity(FINGERPRINT_PREFIX.len() + digest.len() * 2);
        value.push_str(FINGERPRINT_PREFIX);
        for byte in digest {
            value.push(HEX[usize::from(byte >> 4)] as char);
            value.push(HEX[usize::from(byte & 0x0f)] as char);
        }
        Self(value)
    }
}

impl fmt::Display for DataFingerprint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Provenance reported alongside immutable [`super::GameData`].
///
/// The version is optional user/source metadata and is deliberately excluded
/// from the fingerprint. The fingerprint identifies the exact bytes and
/// presence of the five fixed RePoE JSON inputs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DataProvenance {
    repoe_version: Option<String>,
    fingerprint: DataFingerprint,
}

impl DataProvenance {
    /// Construct provenance without a claimed RePoE version.
    pub fn unversioned(fingerprint: DataFingerprint) -> Self {
        Self {
            repoe_version: None,
            fingerprint,
        }
    }

    /// Construct provenance with a normalized, non-empty RePoE version.
    pub fn versioned(fingerprint: DataFingerprint, repoe_version: &str) -> Result<Self> {
        Ok(Self {
            repoe_version: Some(normalize_repoe_version(repoe_version)?),
            fingerprint,
        })
    }

    /// Optional normalized RePoE version supplied by the source bundle.
    pub fn repoe_version(&self) -> Option<&str> {
        self.repoe_version.as_deref()
    }

    /// Stable identity of the loaded source bytes.
    pub fn fingerprint(&self) -> &DataFingerprint {
        &self.fingerprint
    }
}

pub(crate) fn normalize_repoe_version(raw: &str) -> Result<String> {
    let normalized = raw.trim();
    if normalized.is_empty() {
        bail!("RePoE version must not be empty");
    }
    if normalized.chars().any(char::is_control) {
        bail!("RePoE version must not contain control characters");
    }
    Ok(normalized.to_owned())
}

pub(crate) fn fingerprint_repoe_bundle(contents: [Option<&[u8]>; 5]) -> DataFingerprint {
    let mut hasher = Sha256::new();
    hash_framed_bytes(&mut hasher, BUNDLE_HASH_DOMAIN);

    for (filename, content) in REPOE_BUNDLE_FILENAMES.into_iter().zip(contents) {
        hash_framed_bytes(&mut hasher, filename.as_bytes());
        match content {
            None => hasher.update([0]),
            Some(bytes) => {
                hasher.update([1]);
                hash_framed_bytes(&mut hasher, bytes);
            }
        }
    }

    let digest = hasher.finalize();
    DataFingerprint::from_digest(&digest)
}

fn hash_framed_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle_fingerprint(contents: [&[u8]; 5]) -> DataFingerprint {
        fingerprint_repoe_bundle(contents.map(Some))
    }

    #[test]
    fn byte_fingerprint_has_stable_golden_format() {
        let fingerprint = DataFingerprint::sha256_of_bytes(b"abc");
        assert_eq!(
            fingerprint.as_str(),
            "repoe-bundle-v1:sha256:ba7816bf8f01cfea414140de5dae2223\
             b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(fingerprint.to_string(), fingerprint.as_str());
    }

    #[test]
    fn bundle_fingerprint_is_deterministic_and_each_input_matters() {
        let original: [&[u8]; 5] = [b"mods", b"bases", b"bench", b"essences", b"fossils"];
        let expected = bundle_fingerprint(original);
        assert_eq!(bundle_fingerprint(original), expected);

        for index in 0..original.len() {
            let mut mutated = original;
            mutated[index] = b"changed";
            assert_ne!(
                bundle_fingerprint(mutated),
                expected,
                "mutation of {} was not reflected",
                REPOE_BUNDLE_FILENAMES[index]
            );
        }
    }

    #[test]
    fn missing_and_present_empty_inputs_have_distinct_fingerprints() {
        let mut missing = [Some(b"mods".as_slice()), None, None, None, None];
        let missing_fingerprint = fingerprint_repoe_bundle(missing);

        missing[1] = Some(b"");
        assert_ne!(
            fingerprint_repoe_bundle(missing),
            missing_fingerprint,
            "presence marker must distinguish a missing file from an empty file"
        );
    }

    #[test]
    fn fixed_length_framing_prevents_concatenation_collisions() {
        let first: [&[u8]; 5] = [b"a", b"bc", b"", b"", b""];
        let second: [&[u8]; 5] = [b"ab", b"c", b"", b"", b""];
        assert_ne!(bundle_fingerprint(first), bundle_fingerprint(second));
    }

    #[test]
    fn version_is_trimmed_and_excluded_from_the_fingerprint() {
        let fingerprint = DataFingerprint::sha256_of_bytes(b"same data");
        let provenance = DataProvenance::versioned(fingerprint.clone(), "  3.27.0 \r\n")
            .expect("ordinary surrounding whitespace should be normalized");

        assert_eq!(provenance.repoe_version(), Some("3.27.0"));
        assert_eq!(provenance.fingerprint(), &fingerprint);
        assert_eq!(
            DataProvenance::unversioned(fingerprint.clone()).fingerprint(),
            &fingerprint
        );
    }

    #[test]
    fn version_rejects_empty_or_internal_control_characters() {
        let fingerprint = DataFingerprint::sha256_of_bytes(b"data");
        assert!(DataProvenance::versioned(fingerprint.clone(), " \r\n\t").is_err());
        assert!(DataProvenance::versioned(fingerprint.clone(), "3.27\0fork").is_err());
        assert!(DataProvenance::versioned(fingerprint, "3.27\nfork").is_err());
    }
}
