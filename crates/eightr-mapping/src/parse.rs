use std::fmt;

use crate::metadata;
use crate::{ClassMapping, FieldMapping, Mapping, MemberKind, MemberMapping, MetadataLine, MethodMapping, OriginalRange};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// 1-based line number.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mapping line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ParseError {}

fn err<T>(line: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError { line, message: message.into() })
}

pub fn parse(text: &str) -> Result<Mapping, ParseError> {
    let mut m = Mapping::default();
    for (i, raw) in text.lines().enumerate() {
        let lineno = i + 1;
        let raw = raw.strip_suffix('\r').unwrap_or(raw);
        if raw.trim().is_empty() {
            continue;
        }
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        let t = raw.trim();

        if let Some(comment) = t.strip_prefix('#') {
            let Some(class) = m.classes.last_mut() else {
                m.preamble.push(raw.to_string());
                continue;
            };
            let body = comment.trim();
            let (parsed, warning) = if body.starts_with('{') {
                metadata::parse(body)
            } else {
                (crate::Metadata::Unknown(serde_json::Value::String(body.to_string())), Some("non-JSON comment".to_string()))
            };
            if let Some(w) = warning {
                m.warnings.push((lineno, w));
            }
            let md = MetadataLine { raw: body.to_string(), parsed };
            match class.members.last_mut() {
                Some(member) if indented => member.metadata.push(md),
                _ => class.metadata.push(md),
            }
            continue;
        }

        if !indented {
            let Some(head) = t.strip_suffix(':') else {
                return err(lineno, "class line must end with ':'");
            };
            let Some((original, obfuscated)) = head.split_once(" -> ") else {
                return err(lineno, "class line missing ' -> '");
            };
            m.classes.push(ClassMapping {
                line: lineno,
                original: original.trim().to_string(),
                obfuscated: obfuscated.trim().to_string(),
                metadata: Vec::new(),
                members: Vec::new(),
            });
            continue;
        }

        let Some(class) = m.classes.last_mut() else {
            return err(lineno, "member line before any class line");
        };
        let kind = parse_member(t).map_err(|message| ParseError { line: lineno, message })?;
        class.members.push(MemberMapping { line: lineno, kind, metadata: Vec::new() });
    }
    Ok(m)
}

fn parse_u32(s: &str, what: &str) -> Result<u32, String> {
    s.parse().map_err(|_| format!("invalid {what}: {s:?}"))
}

fn split_owner(qualified: &str) -> (Option<String>, String) {
    match qualified.rsplit_once('.') {
        Some((owner, name)) => (Some(owner.to_string()), name.to_string()),
        None => (None, qualified.to_string()),
    }
}

fn parse_member(t: &str) -> Result<MemberKind, String> {
    let (lhs, obfuscated) = t.rsplit_once(" -> ").ok_or("member line missing ' -> '")?;
    let obfuscated = obfuscated.trim().to_string();
    if obfuscated.is_empty() {
        return Err("empty obfuscated name".into());
    }
    let mut lhs = lhs.trim();

    // Optional `a:b:` minified range. (Types can't start with a digit, so a leading digit
    // always means a range.)
    let mut minified_range = None;
    if lhs.starts_with(|c: char| c.is_ascii_digit()) {
        let mut parts = lhs.splitn(3, ':');
        let a = parts.next().unwrap_or_default();
        let b = parts.next().ok_or("bad minified range")?;
        lhs = parts.next().ok_or("bad minified range")?;
        let (a, b) = (parse_u32(a, "range start")?, parse_u32(b, "range end")?);
        if a > b {
            return Err(format!("minified range {a}:{b} is backwards"));
        }
        minified_range = Some((a, b));
    }

    let (ty, rest) = lhs.split_once(' ').ok_or("member line missing type")?;
    let rest = rest.trim();

    let Some(open) = rest.find('(') else {
        if minified_range.is_some() {
            return Err("field mapping with a line range".into());
        }
        let (original_owner, original_name) = split_owner(rest);
        return Ok(MemberKind::Field(FieldMapping { ty: ty.to_string(), original_owner, original_name, obfuscated }));
    };
    let close = rest.rfind(')').filter(|&c| c > open).ok_or("unbalanced parentheses")?;
    let (original_owner, original_name) = split_owner(&rest[..open]);
    let params_str = &rest[open + 1..close];
    let params = if params_str.is_empty() { Vec::new() } else { params_str.split(',').map(str::to_string).collect() };
    let tail = &rest[close + 1..];
    let original_range = if tail.is_empty() {
        None
    } else {
        let tail = tail.strip_prefix(':').ok_or_else(|| format!("unexpected text after ')': {tail:?}"))?;
        Some(match tail.split_once(':') {
            Some((c, d)) => OriginalRange::Range(parse_u32(c, "original line")?, parse_u32(d, "original line")?),
            None => OriginalRange::Single(parse_u32(tail, "original line")?),
        })
    };
    Ok(MemberKind::Method(MethodMapping {
        minified_range,
        return_type: ty.to_string(),
        original_owner,
        original_name,
        params,
        original_range,
        obfuscated,
    }))
}
