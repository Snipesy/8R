//! A minimal JVM class-file writer for generated scenario code: straight-line static methods
//! (no branches, so no stack map frames are needed) and `native` declarations.

use std::collections::BTreeMap;

#[derive(Default)]
pub struct ClassWriter {
    cp: Vec<Vec<u8>>,
    index: BTreeMap<Vec<u8>, u16>,
    methods: Vec<Vec<u8>>,
}

pub const ACC_PUBLIC: u16 = 0x0001;
pub const ACC_STATIC: u16 = 0x0008;
pub const ACC_FINAL: u16 = 0x0010;
pub const ACC_SUPER: u16 = 0x0020;
pub const ACC_NATIVE: u16 = 0x0100;

impl ClassWriter {
    fn entry(&mut self, e: Vec<u8>) -> u16 {
        if let Some(&i) = self.index.get(&e) {
            return i;
        }
        self.cp.push(e.clone());
        let i = self.cp.len() as u16;
        self.index.insert(e, i);
        i
    }

    pub fn utf8(&mut self, s: &str) -> u16 {
        let mut e = vec![1];
        e.extend((s.len() as u16).to_be_bytes());
        e.extend(s.as_bytes());
        self.entry(e)
    }

    /// A class by internal name (`a/b/C`) or array descriptor (`[I`).
    pub fn class(&mut self, name: &str) -> u16 {
        let n = self.utf8(name);
        let mut e = vec![7];
        e.extend(n.to_be_bytes());
        self.entry(e)
    }

    pub fn integer(&mut self, v: i32) -> u16 {
        let mut e = vec![3];
        e.extend(v.to_be_bytes());
        self.entry(e)
    }

    pub fn method_ref(&mut self, owner: &str, name: &str, desc: &str, interface: bool) -> u16 {
        let c = self.class(owner);
        let (n, d) = (self.utf8(name), self.utf8(desc));
        let mut nat = vec![12];
        nat.extend(n.to_be_bytes());
        nat.extend(d.to_be_bytes());
        let nat = self.entry(nat);
        let mut e = vec![if interface { 11 } else { 10 }];
        e.extend(c.to_be_bytes());
        e.extend(nat.to_be_bytes());
        self.entry(e)
    }

    /// Adds a method; `code` is (max stack, max locals, bytecode).
    pub fn method(&mut self, access: u16, name: &str, desc: &str, code: Option<(u16, u16, Vec<u8>)>) {
        let (n, d) = (self.utf8(name), self.utf8(desc));
        let mut m = Vec::new();
        m.extend(access.to_be_bytes());
        m.extend(n.to_be_bytes());
        m.extend(d.to_be_bytes());
        match code {
            None => m.extend(0u16.to_be_bytes()),
            Some((stack, locals, bytes)) => {
                let attr = self.utf8("Code");
                m.extend(1u16.to_be_bytes());
                m.extend(attr.to_be_bytes());
                m.extend((12 + bytes.len() as u32).to_be_bytes());
                m.extend(stack.to_be_bytes());
                m.extend(locals.to_be_bytes());
                m.extend((bytes.len() as u32).to_be_bytes());
                m.extend(bytes);
                m.extend(0u16.to_be_bytes()); // exception table
                m.extend(0u16.to_be_bytes()); // attributes
            }
        }
        self.methods.push(m);
    }

    /// The class file (Java 8, no interfaces or fields).
    pub fn finish(mut self, this: &str, superclass: &str, access: u16) -> Vec<u8> {
        let (t, s) = (self.class(this), self.class(superclass));
        let mut out = vec![0xCA, 0xFE, 0xBA, 0xBE, 0, 0, 0, 52];
        out.extend((self.cp.len() as u16 + 1).to_be_bytes());
        for e in &self.cp {
            out.extend(e);
        }
        out.extend(access.to_be_bytes());
        out.extend(t.to_be_bytes());
        out.extend(s.to_be_bytes());
        out.extend(0u16.to_be_bytes()); // interfaces
        out.extend(0u16.to_be_bytes()); // fields
        out.extend((self.methods.len() as u16).to_be_bytes());
        for m in &self.methods {
            out.extend(m);
        }
        out.extend(0u16.to_be_bytes()); // attributes
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_classes_parse_back() {
        let mut w = ClassWriter::default();
        w.method(ACC_PUBLIC | ACC_STATIC | ACC_NATIVE, "i", "()I", None);
        let r = w.method_ref("gen/O", "i", "()I", false);
        let mut code = vec![0xb8];
        code.extend(r.to_be_bytes());
        code.extend([0x57, 0xb1]);
        w.method(ACC_PUBLIC | ACC_STATIC, "run", "()V", Some((1, 0, code)));
        let bytes = w.finish("gen/O", "java/lang/Object", ACC_PUBLIC | ACC_FINAL | ACC_SUPER);
        let c = crate::api::parse_class(&bytes).unwrap();
        assert_eq!(c.descriptor, "Lgen/O;");
        assert_eq!(c.methods.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["i", "run"]);
    }
}
