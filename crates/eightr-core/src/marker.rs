//! D8/R8/L8 markers: JSON strings like `~~R8{"backend":"dex","min-api":21,...}` that the
//! compilers embed in the string pool.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Marker {
    /// `R8`, `D8`, `L8`, or whatever follows `~~`.
    pub tool: String,
    pub fields: BTreeMap<String, Value>,
}

impl Marker {
    pub fn parse(s: &str) -> Option<Marker> {
        let rest = s.strip_prefix("~~")?;
        let brace = rest.find('{')?;
        let tool = &rest[..brace];
        if tool.is_empty() || !tool.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let Value::Object(obj) = serde_json::from_str(&rest[brace..]).ok()? else { return None };
        Some(Marker { tool: tool.to_string(), fields: obj.into_iter().collect() })
    }

    fn str(&self, k: &str) -> Option<&str> {
        self.fields.get(k).and_then(Value::as_str)
    }

    pub fn version(&self) -> Option<&str> {
        self.str("version")
    }
    pub fn min_api(&self) -> Option<u64> {
        self.fields.get("min-api").and_then(Value::as_u64)
    }
    pub fn compilation_mode(&self) -> Option<&str> {
        self.str("compilation-mode")
    }
    /// `full` or `compatibility`.
    pub fn r8_mode(&self) -> Option<&str> {
        self.str("r8-mode")
    }
    /// Short hash identifying the mapping file R8 produced alongside this build.
    pub fn pg_map_id(&self) -> Option<&str> {
        self.str("pg-map-id")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_r8_marker() {
        let m = Marker::parse(r#"~~R8{"backend":"dex","compilation-mode":"release","has-checksums":false,"min-api":21,"pg-map-id":"3b65028","r8-mode":"full","version":"8.10.9-dev"}"#).unwrap();
        assert_eq!(m.tool, "R8");
        assert_eq!(m.min_api(), Some(21));
        assert_eq!(m.r8_mode(), Some("full"));
        assert_eq!(m.pg_map_id(), Some("3b65028"));
        assert_eq!(m.version(), Some("8.10.9-dev"));
    }

    #[test]
    fn rejects_non_markers() {
        assert_eq!(Marker::parse("~~R8"), None);
        assert_eq!(Marker::parse("~~R8{not json"), None);
        assert_eq!(Marker::parse("hello ~~R8{}"), None);
        assert_eq!(Marker::parse("~~R8[1]"), None);
        assert_eq!(Marker::parse("~~{}"), None);
    }
}
