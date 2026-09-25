//! String interning. Ids are assigned in first-seen order, which depends on input order, so
//! **a `Sym`'s numeric value must never reach output or influence ordering**. Compare and sort
//! by the string ([`Interner::get`]) instead. `Sym` deliberately does not implement `Ord`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sym(u32);

#[derive(Debug, Default, Clone)]
pub struct Interner {
    strings: Vec<Box<str>>,
    ids: BTreeMap<Box<str>, Sym>,
}

impl Interner {
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&id) = self.ids.get(s) {
            return id;
        }
        let id = Sym(self.strings.len() as u32);
        self.strings.push(s.into());
        self.ids.insert(s.into(), id);
        id
    }

    pub fn get(&self, sym: Sym) -> &str {
        &self.strings[sym.0 as usize]
    }

    pub fn lookup(&self, s: &str) -> Option<Sym> {
        self.ids.get(s).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning() {
        let mut i = Interner::default();
        let a = i.intern("La;");
        let b = i.intern("Lb;");
        assert_eq!(i.intern("La;"), a);
        assert_ne!(a, b);
        assert_eq!(i.get(b), "Lb;");
        assert_eq!(i.lookup("La;"), Some(a));
        assert_eq!(i.lookup("Lc;"), None);
    }
}
