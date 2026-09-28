//! The scenario catalog: data, not code (`data/scenarios.conf` is the default).

use crate::tools::Result;

pub const DEFAULT: &str = include_str!("../data/scenarios.conf");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    LibAlone,
    Roots,
    Callers,
    Defaults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Declared,
    Closure,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub kind: Kind,
    pub name: String,
    pub frac: f64,
    pub seed: u64,
    pub scope: Scope,
}

pub fn parse(text: &str) -> Result<Vec<Scenario>> {
    let mut out: Vec<Scenario> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let w: Vec<&str> = line.split_whitespace().collect();
        let err = |m: &str| format!("scenarios:{}: {m}", n + 1);
        let kind = match w[0] {
            "lib-alone" => Kind::LibAlone,
            "roots" => Kind::Roots,
            "callers" => Kind::Callers,
            "defaults" => Kind::Defaults,
            k => return Err(err(&format!("unknown kind {k}"))),
        };
        let name = w.get(1).ok_or_else(|| err("missing name"))?.to_string();
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') || out.iter().any(|s| s.name == name) {
            return Err(err("names are unique [A-Za-z0-9_-]+"));
        }
        let mut s = Scenario { kind, name, frac: 0.05, seed: 1, scope: Scope::Declared };
        for kv in &w[2..] {
            let (k, v) = kv.split_once('=').ok_or_else(|| err(&format!("expected key=value, got {kv}")))?;
            match k {
                "frac" => s.frac = v.parse().ok().filter(|f: &f64| *f > 0.0 && *f <= 1.0).ok_or_else(|| err("frac in (0, 1]"))?,
                "seed" => s.seed = v.parse().map_err(|_| err("seed is an integer"))?,
                "scope" => {
                    s.scope = match v {
                        "declared" => Scope::Declared,
                        "closure" => Scope::Closure,
                        _ => return Err(err("scope is declared or closure")),
                    }
                }
                _ => return Err(err(&format!("unknown key {k}"))),
            }
        }
        out.push(s);
    }
    if out.len() > 64 {
        return Err("at most 64 scenarios per pack".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_catalog_parses() {
        let s = parse(DEFAULT).unwrap();
        assert!(s.len() >= 3 && s[0].kind == Kind::LibAlone);
        assert!(parse("roots a frac=2").is_err());
        assert!(parse("roots a\nroots a").is_err());
        assert_eq!(parse("callers c frac=0.1 seed=7 scope=closure").unwrap()[0], Scenario { kind: Kind::Callers, name: "c".into(), frac: 0.1, seed: 7, scope: Scope::Closure });
    }
}
