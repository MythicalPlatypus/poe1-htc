use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Opaque, stable identity for one semantic crafting operation.
///
/// IDs are slash-separated. The first two segments are lowercase semantic
/// family/operation names. Remaining configuration segments are canonical
/// percent-encoded UTF-8. Display names and prices are deliberately excluded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MethodId(Arc<str>);

impl MethodId {
    /// Parse and validate an externally supplied method ID.
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidMethodId> {
        let value = value.into();
        validate_method_id(&value)?;
        Ok(Self(value.into()))
    }

    /// The canonical string form used by saved requests and result paths.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Construct an ID from semantic raw UTF-8 segments.
    ///
    /// Family and operation are fixed internal tokens. Every configuration
    /// segment is encoded injectively, so delimiter characters in RePoE IDs or
    /// canonical composite configuration cannot create collisions.
    pub(crate) fn semantic(family: &str, operation: &str, configuration: &[&str]) -> Self {
        assert!(
            is_fixed_segment(family) && is_fixed_segment(operation),
            "MethodId family and operation must be lowercase kebab-case"
        );
        let mut value = String::new();
        value.push_str(family);
        value.push('/');
        value.push_str(operation);
        for segment in configuration {
            value.push('/');
            push_encoded_segment(&mut value, segment);
        }
        Self(value.into())
    }
}

impl fmt::Display for MethodId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl AsRef<str> for MethodId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for MethodId {
    type Err = InvalidMethodId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl Serialize for MethodId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for MethodId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(de::Error::custom)
    }
}

/// Why a serialized or caller-supplied method ID was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMethodId {
    message: String,
}

impl InvalidMethodId {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for InvalidMethodId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for InvalidMethodId {}

fn validate_method_id(value: &str) -> Result<(), InvalidMethodId> {
    let mut segments = value.split('/');
    let family = segments.next().ok_or_else(|| {
        InvalidMethodId::new("method ID must contain family and operation segments")
    })?;
    let Some(operation) = segments.next() else {
        return Err(InvalidMethodId::new(
            "method ID must contain at least family and operation segments",
        ));
    };
    for (position, segment) in [family, operation].into_iter().enumerate() {
        if !is_fixed_segment(segment) {
            return Err(InvalidMethodId::new(format!(
                "method ID segment {position} must be non-empty lowercase kebab-case"
            )));
        }
    }
    validate_canonical_segment(family)?;
    validate_canonical_segment(operation)?;
    for segment in segments {
        validate_canonical_segment(segment)?;
    }
    Ok(())
}

fn is_fixed_segment(segment: &str) -> bool {
    segment.split('-').all(|word| {
        !word.is_empty()
            && word
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn validate_canonical_segment(segment: &str) -> Result<(), InvalidMethodId> {
    let input = segment.as_bytes();
    let mut decoded = Vec::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let byte = input[index];
        if is_unreserved(byte) {
            decoded.push(byte);
            index += 1;
            continue;
        }
        if byte != b'%' || index + 2 >= input.len() {
            return Err(InvalidMethodId::new(
                "method ID contains a character outside canonical percent encoding",
            ));
        }
        let high = decode_upper_hex(input[index + 1]).ok_or_else(|| {
            InvalidMethodId::new("method ID percent escapes must use uppercase hexadecimal")
        })?;
        let low = decode_upper_hex(input[index + 2]).ok_or_else(|| {
            InvalidMethodId::new("method ID percent escapes must use uppercase hexadecimal")
        })?;
        decoded.push((high << 4) | low);
        index += 3;
    }

    let decoded = std::str::from_utf8(&decoded)
        .map_err(|_| InvalidMethodId::new("method ID percent escapes are not valid UTF-8"))?;
    let mut canonical = String::new();
    push_encoded_segment(&mut canonical, decoded);
    if canonical != segment {
        return Err(InvalidMethodId::new(
            "method ID segment is not canonically percent-encoded",
        ));
    }
    Ok(())
}

fn push_encoded_segment(output: &mut String, value: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in value.as_bytes() {
        if is_unreserved(*byte) {
            output.push(char::from(*byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 0x0F) as usize]));
        }
    }
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn decode_upper_hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynamic_segments_are_injective_percent_encoded_utf8() {
        let embedded_separator =
            MethodId::semantic("bench", "add-explicit", &["Metadata/Mods/Élite%Life"]);
        let separate_segments =
            MethodId::semantic("bench", "add-explicit", &["Metadata", "Mods", "Élite%Life"]);

        assert_eq!(
            embedded_separator.as_str(),
            "bench/add-explicit/Metadata%2FMods%2F%C3%89lite%25Life"
        );
        assert_ne!(embedded_separator, separate_segments);
    }

    #[test]
    fn serde_round_trip_preserves_the_opaque_canonical_value() {
        let method_id = MethodId::semantic("essence", "apply", &["Essence/生命"]);
        let json = serde_json::to_string(&method_id).expect("MethodId should serialize");
        let round_trip: MethodId =
            serde_json::from_str(&json).expect("serialized MethodId should validate");

        assert_eq!(round_trip, method_id);
    }

    #[test]
    fn clones_share_the_immutable_backing_value() {
        let method_id = MethodId::semantic("currency", "chaos", &[]);
        let cloned = method_id.clone();

        assert!(Arc::ptr_eq(&method_id.0, &cloned.0));
    }

    #[test]
    fn deserialization_applies_the_same_validation_as_parse() {
        let error = serde_json::from_str::<MethodId>(r#""currency/chaos/%2f""#)
            .expect_err("noncanonical percent encoding must be rejected");
        assert!(
            error.to_string().contains("uppercase hexadecimal"),
            "unexpected serde validation error: {error}"
        );
        assert!(
            serde_json::from_str::<MethodId>("42").is_err(),
            "MethodId must deserialize from a string"
        );
    }

    #[test]
    fn validation_rejects_noncanonical_or_malformed_ids() {
        for invalid in [
            "",
            "chaos",
            "/chaos",
            "currency/",
            "Currency/chaos",
            "-currency/chaos",
            "currency-/chaos",
            "curr--ency/chaos",
            "currency/-chaos",
            "currency/chaos-",
            "currency/chaos--orb",
            "currency/chaos mod",
            "currency/chaos/生命",
            "currency/chaos/%2f",
            "currency/chaos/%41",
            "currency/chaos/%FF",
        ] {
            assert!(
                MethodId::parse(invalid).is_err(),
                "{invalid:?} should be rejected"
            );
        }
    }
}
