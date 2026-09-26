//! Parser and printer for R8/ProGuard mapping files.
//!
//! Format reference: R8's `doc/retrace.md` ("R8, Retrace and map file versioning").
//!
//! ```text
//! # {"id":"com.android.tools.r8.mapping","version":"2.2"}
//! com.example.Main -> a.a:
//! # {"id":"sourceFile","fileName":"Main.java"}
//!     int count -> a
//!     1:1:void inlined():42:42 -> b
//!     1:1:void caller():10 -> b
//!       # {"id":"com.android.tools.r8.residualsignature","signature":"()V"}
//! ```
//!
//! The parser is lossless for R8-generated files: `mapping.to_string() == input`.

mod metadata;
mod parse;

use std::collections::BTreeMap;
use std::fmt;

pub use metadata::Metadata;
pub use parse::ParseError;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mapping {
    /// Every line before the first class mapping, verbatim (compiler info, version, map id).
    pub preamble: Vec<String>,
    pub classes: Vec<ClassMapping>,
    /// Non-fatal problems (e.g. unparseable metadata JSON), with 1-based line numbers.
    pub warnings: Vec<(usize, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassMapping {
    pub line: usize,
    /// Original (source) name, dotted: `com.example.Main$Inner`.
    pub original: String,
    /// Residual (obfuscated) name, dotted.
    pub obfuscated: String,
    pub metadata: Vec<MetadataLine>,
    /// Fields and methods, in file order.
    pub members: Vec<MemberMapping>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberMapping {
    pub line: usize,
    pub kind: MemberKind,
    pub metadata: Vec<MetadataLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MemberKind {
    Field(FieldMapping),
    Method(MethodMapping),
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldMapping {
    pub ty: String,
    /// Set when the field originally belonged to another class (e.g. after class merging).
    pub original_owner: Option<String>,
    pub original_name: String,
    pub obfuscated: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodMapping {
    /// `a:b:` prefix: range of residual line numbers (or pcs) this entry covers.
    pub minified_range: Option<(u32, u32)>,
    pub return_type: String,
    /// Set when the code originally belonged to another class: inlined frames, merged classes.
    pub original_owner: Option<String>,
    pub original_name: String,
    pub params: Vec<String>,
    pub original_range: Option<OriginalRange>,
    pub obfuscated: String,
}

/// The `:c` or `:c:d` suffix of a method line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginalRange {
    /// `:c` — a single line. For an outer inline frame, this is the call-site line.
    Single(u32),
    /// `:c:d` — maps the minified range linearly onto `c..=d` (or collapses onto `c` when
    /// the spans differ).
    Range(u32, u32),
}

/// A `# {...}` comment attached to a class or member, kept verbatim alongside its parse.
#[derive(Debug, Clone, PartialEq)]
pub struct MetadataLine {
    pub raw: String,
    pub parsed: Metadata,
}

impl MethodMapping {
    /// Original line for residual line `line`, if this entry covers it.
    pub fn original_line(&self, line: u32) -> Option<u32> {
        let (a, b) = self.minified_range?;
        if line < a || line > b {
            return None;
        }
        Some(match self.original_range {
            None => line,
            Some(OriginalRange::Single(c)) => c,
            Some(OriginalRange::Range(c, d)) => {
                // Linear when the spans have equal length; otherwise R8 maps everything to c.
                if d - c == b - a { c + (line - a) } else { c }
            }
        })
    }

    /// `void foo(int,java.lang.String)` style signature (no owner).
    pub fn signature(&self) -> String {
        format!("{} {}({})", self.return_type, self.original_name, self.params.join(","))
    }
}

/// One frame of a retraced position: innermost frame first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Original class (dotted).
    pub class: String,
    pub method: String,
    pub signature: String,
    pub line: Option<u32>,
}

impl ClassMapping {
    pub fn fields(&self) -> impl Iterator<Item = &FieldMapping> {
        self.members.iter().filter_map(|m| match &m.kind {
            MemberKind::Field(f) => Some(f),
            _ => None,
        })
    }

    pub fn methods(&self) -> impl Iterator<Item = (&MethodMapping, &[MetadataLine])> {
        self.members.iter().filter_map(|m| match &m.kind {
            MemberKind::Method(x) => Some((x, m.metadata.as_slice())),
            _ => None,
        })
    }

    /// Method entries that describe residual methods themselves, i.e. the outermost frame of
    /// each inline stack (skipping R8's synthesized same-name wrapper frames). R8 writes a stack as consecutive lines sharing a minified range,
    /// innermost first, so the last line of each run is the outermost frame. Entries without
    /// a range stand alone.
    pub fn outermost_methods(&self) -> Vec<(&MethodMapping, &[MetadataLine])> {
        let all: Vec<_> = self.methods().collect();
        let mut out = Vec::new();
        for (i, &(m, md)) in all.iter().enumerate() {
            let continues = m.minified_range.is_some()
                && all.get(i + 1).is_some_and(|(next, _)| {
                    next.obfuscated == m.obfuscated && next.minified_range == m.minified_range
                });
            if !continues {
                // R8 ≥ 9 wraps a method it moved or bridged in a synthesized frame of the same
                // name (`int Big.hashCode():0` around the real `int hashCode()`): the method is
                // the frame inside it.
                let wrapper = md.iter().any(|x| x.parsed == Metadata::Synthesized)
                    && i > 0
                    && all.get(i - 1).is_some_and(|(prev, _)| {
                        prev.obfuscated == m.obfuscated && prev.minified_range == m.minified_range && prev.original_name == m.original_name
                    });
                out.push(if wrapper { all[i - 1] } else { (m, md) });
            }
        }
        out
    }

    pub fn is_synthesized(&self) -> bool {
        self.metadata.iter().any(|m| m.parsed == Metadata::Synthesized)
    }

    pub fn source_file(&self) -> Option<&str> {
        self.metadata.iter().find_map(|m| match &m.parsed {
            Metadata::SourceFile(f) => Some(f.as_str()),
            _ => None,
        })
    }

    /// Inline frame stack for residual method `obfuscated` at residual line `line`,
    /// innermost first. Empty if nothing covers that position.
    ///
    /// Consecutive lines with the same minified range form one stack (R8 writes the
    /// innermost frame first and each caller below it).
    pub fn frames(&self, obfuscated: &str, line: u32) -> Vec<Frame> {
        let mut out = Vec::new();
        let mut current_range = None;
        for (m, _) in self.methods() {
            if m.obfuscated != obfuscated {
                continue;
            }
            let Some(orig) = m.original_line(line) else {
                // A different range: if we already collected a stack, it's complete.
                if current_range.is_some() {
                    break;
                }
                continue;
            };
            if current_range.is_some() && current_range != m.minified_range {
                break;
            }
            current_range = m.minified_range;
            out.push(Frame {
                class: m.original_owner.clone().unwrap_or_else(|| self.original.clone()),
                method: m.original_name.clone(),
                signature: m.signature(),
                line: Some(orig),
            });
        }
        out
    }
}

/// Header fields R8 writes as `# key: value` comments.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeaderInfo {
    pub compiler: Option<String>,
    pub compiler_version: Option<String>,
    pub min_api: Option<u32>,
    pub pg_map_id: Option<String>,
    pub pg_map_hash: Option<String>,
    pub version: Option<String>,
}

impl Mapping {
    pub fn parse(text: &str) -> Result<Mapping, ParseError> {
        parse::parse(text)
    }

    /// Parses and normalizes R8 ≥ 9's quirks (package-relative names in synthesized frames;
    /// `residualsignature` written only on a method's first range). For consumers of the
    /// mapping's meaning; `parse` keeps the text exactly.
    pub fn parse_normalized(text: &str) -> Result<Mapping, ParseError> {
        parse::parse_normalized(text)
    }

    pub fn header(&self) -> HeaderInfo {
        let mut h = HeaderInfo::default();
        for line in &self.preamble {
            let Some(body) = line.strip_prefix('#').map(str::trim) else { continue };
            if body.starts_with('{') {
                if let Metadata::MapVersion(v) = metadata::parse(body).0 {
                    h.version = Some(v);
                }
                continue;
            }
            let Some((k, v)) = body.split_once(':') else { continue };
            let v = v.trim().to_string();
            match k.trim() {
                "compiler" => h.compiler = Some(v),
                "compiler_version" => h.compiler_version = Some(v),
                "min_api" => h.min_api = v.parse().ok(),
                "pg_map_id" => h.pg_map_id = Some(v),
                "pg_map_hash" => h.pg_map_hash = Some(v),
                _ => {}
            }
        }
        h
    }

    /// Index of residual class name → position in `classes`.
    pub fn by_obfuscated(&self) -> BTreeMap<&str, usize> {
        self.classes.iter().enumerate().map(|(i, c)| (c.obfuscated.as_str(), i)).collect()
    }

    /// Index of original class name → position in `classes`.
    pub fn by_original(&self) -> BTreeMap<&str, usize> {
        self.classes.iter().enumerate().map(|(i, c)| (c.original.as_str(), i)).collect()
    }
}

impl fmt::Display for Mapping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for line in &self.preamble {
            writeln!(f, "{line}")?;
        }
        for c in &self.classes {
            writeln!(f, "{} -> {}:", c.original, c.obfuscated)?;
            for m in &c.metadata {
                writeln!(f, "# {}", m.raw)?;
            }
            for m in &c.members {
                write!(f, "    ")?;
                match &m.kind {
                    MemberKind::Field(x) => {
                        write!(f, "{} ", x.ty)?;
                        if let Some(o) = &x.original_owner {
                            write!(f, "{o}.")?;
                        }
                        writeln!(f, "{} -> {}", x.original_name, x.obfuscated)?;
                    }
                    MemberKind::Method(x) => {
                        if let Some((a, b)) = x.minified_range {
                            write!(f, "{a}:{b}:")?;
                        }
                        write!(f, "{} ", x.return_type)?;
                        if let Some(o) = &x.original_owner {
                            write!(f, "{o}.")?;
                        }
                        write!(f, "{}({})", x.original_name, x.params.join(","))?;
                        match x.original_range {
                            Some(OriginalRange::Single(c)) => write!(f, ":{c}")?,
                            Some(OriginalRange::Range(c, d)) => write!(f, ":{c}:{d}")?,
                            None => {}
                        }
                        writeln!(f, " -> {}", x.obfuscated)?;
                    }
                }
                for md in &m.metadata {
                    writeln!(f, "      # {}", md.raw)?;
                }
            }
        }
        Ok(())
    }
}
