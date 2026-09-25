use std::collections::BTreeMap;

use serde_json::Value;

/// Parsed `# {...}` metadata. Unknown ids are preserved as `Unknown`, never dropped.
#[derive(Debug, Clone, PartialEq)]
pub enum Metadata {
    /// `com.android.tools.r8.mapping`
    MapVersion(String),
    /// `sourceFile`
    SourceFile(String),
    /// `com.android.tools.r8.synthesized`
    Synthesized,
    /// `com.android.tools.r8.residualsignature`
    ResidualSignature(String),
    /// `com.android.tools.r8.rewriteFrame`
    RewriteFrame { conditions: Vec<String>, actions: Vec<String> },
    /// `com.android.tools.r8.outline`
    Outline,
    /// `com.android.tools.r8.outlineCallsite`: outline position → callsite position.
    OutlineCallsite { positions: BTreeMap<u32, u32>, outline: Option<String> },
    Unknown(Value),
}

/// Parses the JSON body of a metadata comment. Returns the metadata and an optional warning.
///
/// R8 writes strict JSON, but the spec's examples use single quotes and bare keys, and other
/// tools copy them; a lenient fallback accepts those.
pub fn parse(json: &str) -> (Metadata, Option<String>) {
    let value = match serde_json::from_str::<Value>(json) {
        Ok(v) => v,
        Err(e) => match serde_json::from_str::<Value>(&lenient_to_json(json)) {
            Ok(v) => v,
            Err(_) => return (Metadata::Unknown(Value::String(json.to_string())), Some(format!("invalid metadata JSON: {e}"))),
        },
    };
    let str_field = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_string);
    let str_list = |k: &str| -> Vec<String> {
        value
            .get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default()
    };
    let id = str_field("id");
    let md = match id.as_deref() {
        Some("com.android.tools.r8.mapping") => match str_field("version") {
            Some(v) => Metadata::MapVersion(v),
            None => return (Metadata::Unknown(value), Some("mapping version without 'version'".into())),
        },
        Some("sourceFile") => match str_field("fileName") {
            Some(f) => Metadata::SourceFile(f),
            None => return (Metadata::Unknown(value), Some("sourceFile without 'fileName'".into())),
        },
        Some("com.android.tools.r8.synthesized") => Metadata::Synthesized,
        Some("com.android.tools.r8.residualsignature") => match str_field("signature") {
            Some(s) => Metadata::ResidualSignature(s),
            None => return (Metadata::Unknown(value), Some("residualsignature without 'signature'".into())),
        },
        Some("com.android.tools.r8.rewriteFrame") => {
            Metadata::RewriteFrame { conditions: str_list("conditions"), actions: str_list("actions") }
        }
        Some("com.android.tools.r8.outline") => Metadata::Outline,
        Some("com.android.tools.r8.outlineCallsite") => {
            let mut positions = BTreeMap::new();
            if let Some(obj) = value.get("positions").and_then(Value::as_object) {
                for (k, v) in obj {
                    match (k.parse::<u32>(), v.as_u64().and_then(|v| u32::try_from(v).ok())) {
                        (Ok(k), Some(v)) => {
                            positions.insert(k, v);
                        }
                        _ => return (Metadata::Unknown(value.clone()), Some("bad outlineCallsite position".into())),
                    }
                }
            }
            Metadata::OutlineCallsite { positions, outline: str_field("outline") }
        }
        Some(other) => {
            let w = format!("unknown metadata id '{other}' (preserved)");
            return (Metadata::Unknown(value), Some(w));
        }
        None => return (Metadata::Unknown(value), Some("metadata without 'id'".into())),
    };
    (md, None)
}

/// Converts `{ id: 'x', 'k': ['a'] }` to `{"id":"x","k":["a"]}`. Handles quotes and bare
/// identifier keys; does not attempt to be a full JSON5 parser.
fn lenient_to_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' || c == '"' {
            // Copy a quoted string, normalizing the delimiter to '"'.
            out.push('"');
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    out.push(chars[i]);
                    i += 1;
                } else if chars[i] == '"' {
                    out.push('\\');
                }
                out.push(chars[i]);
                i += 1;
            }
            out.push('"');
            i += 1;
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let mut j = i;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            let is_key = j < chars.len() && chars[j] == ':';
            if is_key || !matches!(word.as_str(), "true" | "false" | "null") {
                out.push('"');
                out.push_str(&word);
                out.push('"');
            } else {
                out.push_str(&word);
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_json() {
        assert_eq!(parse(r#"{"id":"sourceFile","fileName":"Main.java"}"#).0, Metadata::SourceFile("Main.java".into()));
        assert_eq!(parse(r#"{"id":"com.android.tools.r8.synthesized"}"#).0, Metadata::Synthesized);
    }

    // The examples in R8's retrace.md, verbatim.
    #[test]
    fn spec_examples_are_lenient_json() {
        assert_eq!(parse("{'id':'com.android.tools.r8.synthesized'}").0, Metadata::Synthesized);
        assert_eq!(
            parse("{ id: 'com.android.tools.r8.rewriteFrame', conditions: ['throws(Ljava/lang/NullPointerException;)'], actions: ['removeInnerFrames(1)'] }").0,
            Metadata::RewriteFrame {
                conditions: vec!["throws(Ljava/lang/NullPointerException;)".into()],
                actions: vec!["removeInnerFrames(1)".into()],
            }
        );
        assert_eq!(parse("{ 'id':'com.android.tools.r8.outline' }").0, Metadata::Outline);
        assert_eq!(
            parse("{ 'id':'com.android.tools.r8.outlineCallsite', 'positions': {'1': 4, '2': 5}, 'outline':'La;a()I' }").0,
            Metadata::OutlineCallsite { positions: [(1, 4), (2, 5)].into(), outline: Some("La;a()I".into()) }
        );
        assert_eq!(
            parse("{ id: 'com.android.tools.r8.residualsignature', signature:'(Z)a' }").0,
            Metadata::ResidualSignature("(Z)a".into())
        );
    }

    #[test]
    fn unknown_and_invalid_are_preserved_with_warning() {
        let (m, w) = parse(r#"{"id":"com.example.future","x":1}"#);
        assert!(matches!(m, Metadata::Unknown(_)));
        assert!(w.unwrap().contains("com.example.future"));
        let (m, w) = parse("{not json");
        assert!(matches!(m, Metadata::Unknown(_)));
        assert!(w.is_some());
    }
}
