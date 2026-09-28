//! Matching an app's code against the signature DB (docs/research/sigdb.md §3): library
//! methods and classes named by fingerprint. Every name is D (a strong hint): the DB is not an
//! exhaustive candidate universe (app code can compile to a library body), and even the exact
//! stage is 97–99% precise, not 100%.
//!
//! 1. Exact: an informative method whose `all` hash, then string-set hash, is unique among the
//!    app's unmatched methods and names one DB method.
//! 2. Class seeds: unique class shapes (C2, then C3) on both sides.
//! 3. Propagation to a fixpoint (≤ 8 rounds), from the state at the start of each round:
//!    call-graph alignment of matched pairs' program callees; class votes (seeds 3, matched
//!    members 2); within voted class pairs, a unique erased proto, then mutual-best sketch
//!    similarity. A proposal is accepted only when neither side has a competing one, so the
//!    result doesn't depend on iteration order (α-invariant).

use std::collections::{BTreeMap, BTreeSet};

use eightr_ir::model::Program as Model;

use super::db::SigDb;
use super::print::{class_print, method_print, platform_stable, reflective_strings, similarity, MethodPrint};

/// The DBs shipped with 8R (`cargo xtask sigdb`).
const EMBEDDED: &[&[u8]] = &[
    include_bytes!("../../../../sigdb/collection.sigdb"),
    include_bytes!("../../../../sigdb/coroutines.sigdb"),
    include_bytes!("../../../../sigdb/okhttp.sigdb"),
    include_bytes!("../../../../sigdb/okio.sigdb"),
    include_bytes!("../../../../sigdb/stdlib.sigdb"),
];

pub fn embedded() -> &'static [SigDb] {
    static DBS: std::sync::OnceLock<Vec<SigDb>> = std::sync::OnceLock::new();
    DBS.get_or_init(|| EMBEDDED.iter().filter_map(|b| SigDb::decode(b).ok()).collect())
}

/// The DBs to match against: LibDB packs forged for this app first, then the embedded DBs
/// without the classes a pack covers (a pack, built from the app's exact library versions and
/// R8, is the better witness; the same method in two DBs would make every exact match ambiguous).
pub fn with_packs(packs: &[SigDb]) -> std::borrow::Cow<'static, [SigDb]> {
    if packs.is_empty() {
        return std::borrow::Cow::Borrowed(embedded());
    }
    let covered: BTreeSet<&str> = packs.iter().flat_map(|db| db.records.iter().map(move |r| db.classes[db.methods[r.method as usize].0 as usize].as_str())).collect();
    let mut out: Vec<SigDb> = packs.to_vec();
    for db in embedded() {
        let mut db = db.clone();
        let keep = |c: u32| !covered.contains(db.classes[c as usize].as_str());
        let records = db.records.iter().filter(|r| keep(db.methods[r.method as usize].0)).cloned().collect();
        let class_records = db.class_records.iter().filter(|r| keep(r.class)).cloned().collect();
        db.records = records;
        db.class_records = class_records;
        out.push(db);
    }
    std::borrow::Cow::Owned(out)
}

/// A DB method: (library index, method index).
pub type Key = (u16, u32);
/// A DB class: (library index, class index).
pub type ClassKey = (u16, u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Match {
    pub class: usize,
    pub method: usize,
    pub key: Key,
    /// How: `exact:all`, `exact:strings`, `callgraph`, `class:proto`, `mutual-best`.
    pub via: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Matches {
    pub methods: Vec<Match>,
    /// App class → DB class (voted).
    pub classes: BTreeMap<usize, ClassKey>,
}

/// Longest common subsequence alignment of two token lists: aligned index pairs.
fn align(a: &[u64], b: &[u64]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    if n == 0 || m == 0 || n * m > 250_000 {
        return Vec::new();
    }
    let mut t = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i][j] = if a[i] == b[j] { t[i + 1][j + 1] + 1 } else { t[i + 1][j].max(t[i][j + 1]) };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[i + 1][j] >= t[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Accepts the proposals whose app method and DB key have no competing proposal.
fn resolve(proposals: &[(usize, Key, &'static str)]) -> Vec<(usize, Key, &'static str)> {
    let mut by_app: BTreeMap<usize, BTreeSet<Key>> = BTreeMap::new();
    let mut by_key: BTreeMap<Key, BTreeSet<usize>> = BTreeMap::new();
    for &(a, k, _) in proposals {
        by_app.entry(a).or_default().insert(k);
        by_key.entry(k).or_default().insert(a);
    }
    let mut out: BTreeMap<(usize, Key), &'static str> = BTreeMap::new();
    for &(a, k, via) in proposals {
        if by_app[&a].len() == 1 && by_key[&k].len() == 1 {
            // The first stage proposing it names it (proposals are pushed strongest first).
            out.entry((a, k)).or_insert(via);
        }
    }
    out.into_iter().map(|((a, k), via)| (a, k, via)).collect()
}

pub fn match_program(p: &Model, dbs: &[SigDb]) -> Matches {
    let s = &p.syms;
    let reflective = reflective_strings(p);
    let stable = |d: &str| platform_stable(d);
    // App methods with code.
    let mut app: Vec<MethodPrint> = Vec::new();
    for (ci, c) in p.classes.iter().enumerate() {
        for mi in 0..c.methods.len() {
            if let Some(mp) = method_print(p, ci, mi, &stable, &reflective) {
                app.push(mp);
            }
        }
    }
    let app_index: BTreeMap<(usize, usize), usize> = app.iter().enumerate().map(|(i, m)| ((m.class, m.method), i)).collect();
    // App callees resolved to app methods (declared in the class or a program superclass).
    let resolve_callee = |c: &str, n: &str, pr: &str| -> Option<usize> {
        let mut k = p.find(c);
        let mut guard = 0;
        while let Some(ci) = k {
            if let Some(mi) = p.classes[ci].methods.iter().position(|m| s.get(m.name) == n && s.get(m.proto) == pr) {
                return app_index.get(&(ci, mi)).copied();
            }
            guard += 1;
            if guard > 64 {
                return None;
            }
            k = p.classes[ci].superclass.and_then(|t| p.find(s.get(t)));
        }
        None
    };
    let app_callees: Vec<Vec<(u64, Option<usize>)>> =
        app.iter().map(|m| m.callees.iter().map(|(tok, (c, n, pr))| (*tok, resolve_callee(c, n, pr))).collect()).collect();

    // DB indexes.
    let mut by_all: BTreeMap<u64, BTreeSet<Key>> = BTreeMap::new();
    let mut by_strings: BTreeMap<u64, BTreeSet<Key>> = BTreeMap::new();
    // Key → record indices (per library).
    let mut records_of: BTreeMap<Key, Vec<usize>> = BTreeMap::new();
    let mut keys_of_class: BTreeMap<ClassKey, Vec<Key>> = BTreeMap::new();
    for (li, db) in dbs.iter().enumerate() {
        let li = li as u16;
        for (ri, r) in db.records.iter().enumerate() {
            let key = (li, r.method);
            records_of.entry(key).or_default().push(ri);
            if r.informative {
                by_all.entry(r.all).or_default().insert(key);
                if r.strings != 0 {
                    by_strings.entry(r.strings).or_default().insert(key);
                }
            }
        }
        for (mi, (c, n, _)) in db.methods.iter().enumerate() {
            if n != "<init>" && n != "<clinit>" {
                keys_of_class.entry((li, *c)).or_default().push((li, mi as u32));
            }
        }
    }
    let key_class = |k: Key| -> ClassKey { (k.0, dbs[k.0 as usize].methods[k.1 as usize].0) };
    // Primitive-kind compatibility of an app method with a DB method (R8 only removes params and
    // may make an instance method static): the app's primitive params are a sub-multiset of the
    // DB's (plus the receiver as a reference), and the returns agree in kind.
    let prims = |proto: &str| -> Option<(BTreeMap<char, usize>, char)> {
        let (ps, r) = eightr_ir::types::parse_proto(proto)?;
        let mut m = BTreeMap::new();
        for t in ps {
            let c = t.chars().next()?;
            if !matches!(c, 'L' | '[') {
                *m.entry(c).or_insert(0) += 1;
            }
        }
        let rk = match r.chars().next()? {
            'L' | '[' => 'L',
            c => c,
        };
        Some((m, rk))
    };
    let app_prims: Vec<Option<(BTreeMap<char, usize>, char)>> = app.iter().map(|m| prims(s.get(p.classes[m.class].methods[m.method].proto))).collect();
    let compatible = |a: usize, k: Key| -> bool {
        let (Some((ap, ar)), Some((dp, dr))) = (&app_prims[a], prims(&dbs[k.0 as usize].methods[k.1 as usize].2)) else { return false };
        (*ar == 'V' || *ar == dr) && ap.iter().all(|(c, n)| dp.get(c).is_some_and(|d| d >= n))
    };
    // `access$x` bridges: R8 inlines the bridged body into them, so the body names the callee.
    let bridge = |k: Key| dbs[k.0 as usize].methods[k.1 as usize].1.starts_with("access$");
    // Constructors and initializers pair only with their own kind (`<init>` names never move).
    let special = |name: &str| name.starts_with('<');
    let app_special: Vec<bool> = app.iter().map(|m| special(s.get(p.classes[m.class].methods[m.method].name))).collect();
    let kind_ok = |a: usize, k: Key| app_special[a] == special(&dbs[k.0 as usize].methods[k.1 as usize].1);
    let key_proto = |k: Key| -> Vec<u64> { records_of.get(&k).map(|rs| rs.iter().map(|&r| dbs[k.0 as usize].records[r].proto).collect()).unwrap_or_default() };
    let sim = |a: usize, k: Key| -> f64 {
        records_of.get(&k).map_or(0.0, |rs| rs.iter().map(|&r| similarity(&app[a].sketch, &dbs[k.0 as usize].records[r].sketch)).fold(0.0, f64::max))
    };

    let mut matched: BTreeMap<usize, (Key, &'static str)> = BTreeMap::new();
    let mut taken: BTreeSet<Key> = BTreeSet::new();

    // 1. Exact cascade.
    for (family, via) in [(&by_all, "exact:all"), (&by_strings, "exact:strings")] {
        let mut app_by: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (i, m) in app.iter().enumerate() {
            if !m.informative || matched.contains_key(&i) {
                continue;
            }
            let h = if via == "exact:all" { m.all } else { m.strings };
            if h != 0 {
                app_by.entry(h).or_default().push(i);
            }
        }
        for (h, apps) in app_by {
            let [a] = apps[..] else { continue };
            let Some(keys) = family.get(&h) else { continue };
            let free: Vec<&Key> = keys.iter().filter(|k| !taken.contains(k)).collect();
            if let ([k], 1) = (free.as_slice(), keys.len()) {
                // A callee inlined into one caller brings its strings along: the string stage also
                // needs compatible primitive kinds.
                if !kind_ok(a, **k) || bridge(**k) || (via == "exact:strings" && !compatible(a, **k)) {
                    continue;
                }
                matched.insert(a, (**k, via));
                taken.insert(**k);
            }
        }
    }

    // 2. Class seeds: unique shapes on both sides (non-trivial app classes only).
    let mut seeds: BTreeMap<usize, ClassKey> = BTreeMap::new();
    {
        let trivial = |ci: usize| {
            let c = &p.classes[ci];
            c.fields.len() < 2 && c.interfaces.iter().all(|t| !stable(s.get(*t))) && c.superclass.is_none_or(|t| s.get(t) == "Ljava/lang/Object;")
        };
        let prints: Vec<_> = (0..p.classes.len()).map(|ci| class_print(p, ci, &stable)).collect();
        for pick in [|x: &super::print::ClassPrint| x.c2, |x: &super::print::ClassPrint| x.c3] {
            let mut app_by: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
            for (ci, cp) in prints.iter().enumerate() {
                if !trivial(ci) && !seeds.contains_key(&ci) {
                    app_by.entry(pick(cp)).or_default().push(ci);
                }
            }
            let mut db_by: BTreeMap<u64, BTreeSet<ClassKey>> = BTreeMap::new();
            for (li, db) in dbs.iter().enumerate() {
                for r in &db.class_records {
                    let v = pick(&super::print::ClassPrint { class: 0, c2: r.c2, c3: r.c3 });
                    db_by.entry(v).or_default().insert((li as u16, r.class));
                }
            }
            let used: BTreeSet<ClassKey> = seeds.values().copied().collect();
            for (h, cs) in app_by {
                let [ci] = cs[..] else { continue };
                if let Some(ks) = db_by.get(&h) {
                    if let [k] = ks.iter().collect::<Vec<_>>()[..] {
                        if !used.contains(k) {
                            seeds.insert(ci, *k);
                        }
                    }
                }
            }
        }
    }

    // 3. Propagation.
    let mut classes: BTreeMap<usize, ClassKey> = BTreeMap::new();
    // Class pairs backed by a seed or at least two matched members: one matched method (e.g. in
    // a merged static holder) doesn't make its class the library's for similarity guesses.
    let mut strong: BTreeSet<usize> = BTreeSet::new();
    for _round in 0..8 {
        // Class votes from the current state.
        let mut votes: BTreeMap<usize, BTreeMap<ClassKey, u32>> = BTreeMap::new();
        for (&ci, &k) in &seeds {
            *votes.entry(ci).or_default().entry(k).or_default() += 3;
        }
        for (&a, &(k, _)) in &matched {
            *votes.entry(app[a].class).or_default().entry(key_class(k)).or_default() += 2;
        }
        classes.clear();
        strong.clear();
        for (ci, v) in &votes {
            let mut ranked: Vec<(&ClassKey, &u32)> = v.iter().collect();
            ranked.sort_by_key(|(k, n)| (std::cmp::Reverse(**n), **k));
            let total: u32 = v.values().sum();
            let (top, n) = ranked[0];
            // A seed, or at least two members: one matched method (e.g. in a merged static holder)
            // doesn't make its class the library's.
            let independent = seeds.get(ci) == Some(top) || *n >= 4;
            let unique = ranked.get(1).is_none_or(|x| x.1 < n);
            if unique && *n * 3 >= total * 2 {
                classes.insert(*ci, *top);
                if independent {
                    strong.insert(*ci);
                }
            }
        }
        let mut proposals: Vec<(usize, Key, &'static str)> = Vec::new();
        // a. Call graph.
        for (&a, &(k, _)) in &matched {
            let Some(rs) = records_of.get(&k) else { continue };
            let db = &dbs[k.0 as usize];
            // The record closest to the app body.
            let r = rs.iter().copied().max_by(|&x, &y| {
                similarity(&app[a].sketch, &db.records[x].sketch).total_cmp(&similarity(&app[a].sketch, &db.records[y].sketch)).then(y.cmp(&x))
            });
            let Some(r) = r else { continue };
            let db_callees = &db.records[r].callees;
            let at: Vec<u64> = app_callees[a].iter().map(|x| x.0).collect();
            let bt: Vec<u64> = db_callees.iter().map(|x| x.0).collect();
            for (i, j) in align(&at, &bt) {
                let (Some(ca), cb) = (app_callees[a][i].1, db_callees[j].1) else { continue };
                if cb == u32::MAX || matched.contains_key(&ca) {
                    continue;
                }
                let kb = (k.0, cb);
                if !taken.contains(&kb) {
                    proposals.push((ca, kb, "callgraph"));
                }
            }
        }
        // b/c. Within voted class pairs.
        let mut app_of_class: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (i, m) in app.iter().enumerate() {
            let n = s.get(p.classes[m.class].methods[m.method].name);
            if !matched.contains_key(&i) && n != "<init>" && n != "<clinit>" {
                app_of_class.entry(m.class).or_default().push(i);
            }
        }
        for (&ci, &dk) in &classes {
            let Some(apps) = app_of_class.get(&ci) else { continue };
            let Some(keys) = keys_of_class.get(&dk) else { continue };
            let keys: Vec<Key> = keys.iter().copied().filter(|k| !taken.contains(k)).collect();
            // Unique erased proto on both sides.
            for &a in apps {
                let pr = app[a].proto;
                let same_app = apps.iter().filter(|&&x| app[x].proto == pr).count();
                let same_db: Vec<Key> = keys.iter().copied().filter(|&k| key_proto(k).contains(&pr)).collect();
                if same_app == 1 && same_db.len() == 1 && sim(a, same_db[0]) >= 0.3 {
                    proposals.push((a, same_db[0], "class:proto"));
                }
            }
            // Mutual best: only in strongly voted class pairs.
            if !strong.contains(&ci) {
                continue;
            }
            let score = |a: usize, k: Key| sim(a, k) + if key_proto(k).contains(&app[a].proto) { 0.2 } else { 0.0 };
            let best_of = |a: usize| -> Option<(Key, f64, f64)> {
                let mut sc: Vec<(f64, Key)> = keys.iter().map(|&k| (score(a, k), k)).collect();
                sc.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
                sc.first().map(|&(b, k)| (k, b, sc.get(1).map_or(0.0, |x| x.0)))
            };
            for &a in apps {
                let Some((k, best, second)) = best_of(a) else { continue };
                if best < 0.35 || best - second < 0.05 {
                    continue;
                }
                // Mutual: no other app method of the class scores within the margin for k.
                let rival = apps.iter().filter(|&&x| x != a).map(|&x| score(x, k)).fold(0.0, f64::max);
                if best - rival >= 0.05 {
                    proposals.push((a, k, "mutual-best"));
                }
            }
        }
        proposals.retain(|&(a, k, via)| kind_ok(a, k) && !bridge(k) && (via == "callgraph" || compatible(a, k)));
        let accepted = resolve(&proposals);
        let mut grew = false;
        for (a, k, via) in accepted {
            if matched.contains_key(&a) || taken.contains(&k) {
                continue;
            }
            matched.insert(a, (k, via));
            taken.insert(k);
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let methods = matched.into_iter().map(|(a, (key, via))| Match { class: app[a].class, method: app[a].method, key, via }).collect();
    // Class pairs reported (hints) only when strongly backed.
    classes.retain(|ci, _| strong.contains(ci));
    Matches { methods, classes }
}
