//! Version 1 chain-document canonicalization for validation namespaces.
//!
//! Object keys are decoded, sorted by UTF-8 bytes and emitted exactly once.
//! Arrays retain order, strings use compact serde_json UTF-8 escaping, and
//! numbers retain their original JSON spelling (no floating-point conversion).
//! Equivalent numeric spellings may therefore conservatively have different
//! identities. Duplicate keys, including escaped aliases, are always rejected.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, de};
use serde_json::value::RawValue;

/// Canonicalizer version; changes to these rules change the identity schema.
pub const CANONICAL_JSON_VERSION: u64 = 1;

/// Canonicalize a complete JSON document without rounding scalar numbers or
/// silently overwriting duplicate keys at any nesting level.
///
/// # Errors
/// Rejects invalid JSON, duplicate object keys, trailing data, and nesting over
/// 128 containers. The caller still validates the chain-document schema.
pub fn canonicalize_chain_json(encoded: &[u8]) -> eyre::Result<Vec<u8>> {
    let value: &RawValue = serde_json::from_slice(encoded)?;
    let mut output = Vec::with_capacity(encoded.len());
    append(value, 0, &mut output)?;
    Ok(output)
}

fn append(value: &RawValue, depth: usize, output: &mut Vec<u8>) -> eyre::Result<()> {
    eyre::ensure!(depth <= 128, "chain document nesting exceeds 128");
    let raw = value.get();
    match raw.as_bytes().first() {
        Some(b'{') => {
            let entries: UniqueObject<'_> = serde_json::from_str(raw)?;
            output.push(b'{');
            for (index, (key, value)) in entries.0.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, &key)?;
                output.push(b':');
                append(value, depth + 1, output)?;
            }
            output.push(b'}');
        }
        Some(b'[') => {
            let entries: Vec<&RawValue> = serde_json::from_str(raw)?;
            output.push(b'[');
            for (index, value) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                append(value, depth + 1, output)?;
            }
            output.push(b']');
        }
        Some(b'"') => {
            let string: String = serde_json::from_str(raw)?;
            serde_json::to_writer(output, &string)?;
        }
        // RawValue has already checked the syntax. Preserve scalar values,
        // including integers outside f64/u64 and distinct negative zero.
        _ => output.extend_from_slice(raw.as_bytes()),
    }
    Ok(())
}

struct UniqueObject<'a>(BTreeMap<String, &'a RawValue>);

impl<'de> Deserialize<'de> for UniqueObject<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> de::Visitor<'de> for Visitor {
            type Value = UniqueObject<'de>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a JSON object with unique keys")
            }

            fn visit_map<M: de::MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut entries = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, &RawValue>()? {
                    if entries.insert(key, value).is_some() {
                        return Err(de::Error::custom("duplicate chain-document object key"));
                    }
                }
                Ok(UniqueObject(entries))
            }
        }
        deserializer.deserialize_map(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_preserves_scalars_and_orders_only_object_keys() {
        let source = br#" { "z": [true, null, -0, 184467440737095516160, 1.234567890123456789, 1e999], "a": { "y": "\u00e9", "b": "\/\n" } } "#;
        let expected = "{\"a\":{\"b\":\"/\\n\",\"y\":\"é\"},\"z\":[true,null,-0,184467440737095516160,1.234567890123456789,1e999]}";
        let actual = canonicalize_chain_json(source).unwrap();
        assert_eq!(actual, expected.as_bytes());
        assert_eq!(canonicalize_chain_json(&actual).unwrap(), actual);
    }

    #[test]
    fn duplicate_keys_are_rejected_even_when_nested_or_escaped() {
        for input in [
            r#"{"chainId":1,"chainId":2}"#,
            r#"{"config":{"chainId":1,"chain\u0049d":1}}"#,
            r#"{"alloc":[{"x":0,"x":0}]}"#,
        ] {
            let error = canonicalize_chain_json(input.as_bytes()).unwrap_err();
            assert!(error.to_string().contains("duplicate"), "{error}");
        }
    }

    #[test]
    fn invalid_json_trailing_data_and_excessive_nesting_fail() {
        for input in ["", "{} {}", "{\"x\":NaN}", "[01]", "[1,]", "\"\\uD800\""] {
            assert!(
                canonicalize_chain_json(input.as_bytes()).is_err(),
                "{input}"
            );
        }
        let nested = format!("{}0{}", "[".repeat(130), "]".repeat(130));
        assert!(canonicalize_chain_json(nested.as_bytes()).is_err());
    }
}
