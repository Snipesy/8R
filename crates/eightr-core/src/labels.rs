//! Per-(item, attribute) faithfulness labels (DESIGN.md §0.2).

use std::collections::BTreeMap;

use eightr_rules::{lookup, Attribute, Class};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::program::ItemId;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Label {
    /// Composition (minimum) of the classes of every rule applied.
    pub class: Class,
    /// The recovered value when a rule changes the attribute (e.g. a recovered field name).
    /// `None` means the current value stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// Rules applied, in application order.
    pub rules: Vec<&'static str>,
    /// For N: the complete candidate set, canonically ordered. The rule guarantees the
    /// original is among them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidates: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct Labels {
    map: BTreeMap<(ItemId, Attribute), Label>,
}

impl Labels {
    /// Records that `rule` vouches for this attribute's current value. `candidates` is
    /// required for N rules and forbidden otherwise.
    pub fn record(
        &mut self,
        item: ItemId,
        attribute: Attribute,
        rule_id: &str,
        candidates: Option<Vec<String>>,
    ) -> Result<()> {
        self.record_value(item, attribute, rule_id, candidates, None)
    }

    /// Like [`Labels::record`], with a recovered value that replaces the current one. Two
    /// rules proposing different values for the same attribute is an internal error: for S
    /// rules it means one of them is unsound.
    pub fn record_value(
        &mut self,
        item: ItemId,
        attribute: Attribute,
        rule_id: &str,
        candidates: Option<Vec<String>>,
        value: Option<String>,
    ) -> Result<()> {
        let bad = |detail: String| Error::UnregisteredRule { rule: rule_id.to_string(), detail };
        let rule = lookup(rule_id).ok_or_else(|| bad("not in registry".into()))?;
        if !rule.attributes.contains(&attribute) {
            return Err(bad(format!("does not declare attribute {attribute:?}")));
        }
        let candidates = match (rule.class, candidates) {
            (Class::NonDeterministic, Some(mut c)) if !c.is_empty() => {
                c.sort();
                c.dedup();
                Some(c)
            }
            (Class::NonDeterministic, _) => return Err(bad("N rule must supply a non-empty candidate set".into())),
            (_, Some(_)) => return Err(bad("only N rules supply candidates".into())),
            (_, None) => None,
        };
        let entry =
            self.map.entry((item, attribute)).or_insert(Label { class: rule.class, value: None, rules: Vec::new(), candidates: None });
        if let (Some(old), Some(new)) = (&entry.value, &value) {
            if old != new {
                return Err(bad(format!("proposes {new:?} but {:?} already proposed {old:?}", entry.rules)));
            }
        }
        if value.is_some() {
            entry.value = value;
        }
        entry.class = entry.class.compose(rule.class);
        entry.rules.push(rule.id);
        if candidates.is_some() {
            entry.candidates = candidates;
        }
        Ok(())
    }

    pub fn get(&self, item: ItemId, attribute: Attribute) -> Option<&Label> {
        self.map.get(&(item, attribute))
    }

    pub fn iter(&self) -> impl Iterator<Item = (&(ItemId, Attribute), &Label)> {
        self.map.iter()
    }
}
