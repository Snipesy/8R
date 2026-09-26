//! Protocol buffer sources bundled in a package (`*.proto`): messages and their fields, to match
//! protobuf-lite generated classes (docs/research/sdk-anchors.md §2b).

/// A message: its full name (`google.firestore.v1.ExistenceFilter`, nested with dots), its own
/// simple name, and its fields (name, number).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub full_name: String,
    pub name: String,
    pub fields: Vec<(String, i64)>,
}

fn tokens(text: &str) -> Vec<String> {
    // Comments out; strings kept as single tokens.
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'"' || c == b'\'' {
            let start = i;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            out.push(text[start..i.min(b.len())].to_string());
        } else if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'.') {
                i += 1;
            }
            out.push(text[start..i].to_string());
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else {
            out.push((c as char).to_string());
            i += 1;
        }
    }
    out
}

/// The messages of one `.proto` file.
pub fn parse(text: &str) -> Vec<Message> {
    let t = tokens(text);
    let mut package = String::new();
    let mut out = Vec::new();
    // Scope stack: Some(message index) or None (a block whose fields are ignored, e.g. enum).
    let mut stack: Vec<Option<usize>> = Vec::new();
    let mut i = 0;
    let current = |stack: &[Option<usize>]| stack.iter().rev().find_map(|x| *x);
    while i < t.len() {
        let w = t[i].as_str();
        match w {
            "package" if stack.is_empty() => {
                package = t.get(i + 1).cloned().unwrap_or_default();
                i += 2;
            }
            "message" if t.get(i + 2).is_some_and(|x| x == "{") => {
                let name = t[i + 1].clone();
                let prefix = match current(&stack) {
                    Some(k) => out.get(k).map(|m: &Message| m.full_name.clone()).unwrap_or_default(),
                    None => package.clone(),
                };
                let full_name = if prefix.is_empty() { name.clone() } else { format!("{prefix}.{name}") };
                out.push(Message { full_name, name, fields: Vec::new() });
                stack.push(Some(out.len() - 1));
                i += 3;
            }
            // Fields inside a oneof belong to the message.
            "oneof" if t.get(i + 2).is_some_and(|x| x == "{") => {
                stack.push(current(&stack));
                i += 3;
            }
            "enum" | "service" | "extend" if t.get(i + 2).is_some_and(|x| x == "{") => {
                stack.push(None);
                i += 3;
            }
            "{" => {
                stack.push(None);
                i += 1;
            }
            "}" => {
                stack.pop();
                i += 1;
            }
            _ => {
                // A statement up to `;`.
                let end = t[i..].iter().position(|x| x == ";" || x == "{" || x == "}").map_or(t.len(), |k| i + k);
                let stmt = &t[i..end];
                let in_message = stack.last().is_some_and(|x| x.is_some());
                if let (true, Some(k)) = (in_message, current(&stack)) {
                    if let Some(eq) = stmt.iter().position(|x| x == "=") {
                        let first = stmt[0].as_str();
                        let skip = matches!(first, "option" | "reserved" | "extensions" | "syntax" | "import");
                        if !skip && eq >= 2 {
                            let name = stmt[eq - 1].clone();
                            if let Some(num) = stmt.get(eq + 1).and_then(|n| n.parse::<i64>().ok()) {
                                out[k].fields.push((name, num));
                            }
                        }
                    }
                }
                i = if t.get(end).is_some_and(|x| x == ";") { end + 1 } else { end.max(i + 1) };
            }
        }
    }
    out
}

/// `target_id` → `TARGET_ID` (protoc's `*_FIELD_NUMBER` constant stem).
pub fn constant_stem(field: &str) -> String {
    field.to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_messages_oneofs_and_enums() {
        let m = parse(
            "syntax = \"proto3\"; package a.b; option java_package = \"x\";\n\
             message Outer { // c\n int32 target_id = 1; repeated string names = 2 [packed=true];\n\
               map<string, int64> counts = 3; oneof kind { bool flag = 4; }\n\
               enum Dir { UP = 0; } message Inner { reserved 2; Outer.Inner x = 1; } }",
        );
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].full_name, "a.b.Outer");
        assert_eq!(m[0].fields, [("target_id".to_string(), 1), ("names".into(), 2), ("counts".into(), 3), ("flag".into(), 4)]);
        assert_eq!((m[1].full_name.as_str(), m[1].name.as_str()), ("a.b.Outer.Inner", "Inner"));
        assert_eq!(m[1].fields, [("x".to_string(), 1)]);
    }
}
