use serde::{Deserialize, Serialize};

use crate::manifest::{CaseManifest, ConformanceLevel};

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Conjunctive selectors applied to prepared conformance cases.
///
/// Empty collections do not restrict selection. Entries within one collection
/// are alternatives, except `tags`, all of which must be present on a case.
pub struct Filters {
    #[serde(default)]
    /// Exact case identifiers to include.
    pub ids: Vec<String>,
    #[serde(default)]
    /// Inclusive numeric case-ID ranges to include.
    pub id_ranges: Vec<IdRange>,
    #[serde(default)]
    /// Specification fragment identifiers to include, with optional `#`.
    pub spec_ids: Vec<String>,
    #[serde(default)]
    /// Specification section numbers to include.
    pub spec_nums: Vec<String>,
    #[serde(default)]
    /// Conformance levels to include.
    pub conformance: Vec<ConformanceLevel>,
    #[serde(default)]
    /// Data-model identifiers to include.
    pub datamodels: Vec<String>,
    #[serde(default)]
    /// Tags every selected case must contain.
    pub tags: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// An inclusive range over the numeric prefix of a case identifier.
pub struct IdRange {
    /// Lowest selected numeric case identifier.
    pub start: u64,
    /// Highest selected numeric case identifier.
    pub end: u64,
}

impl Filters {
    #[must_use]
    /// Returns whether `case` satisfies every configured selector.
    pub fn selects(&self, case: &CaseManifest) -> bool {
        if !self.ids.is_empty() && !self.ids.iter().any(|id| id == &case.id) {
            return false;
        }
        if !self.id_ranges.is_empty()
            && numeric_case_id(&case.id).is_none_or(|id| {
                !self
                    .id_ranges
                    .iter()
                    .any(|range| range.start <= id && id <= range.end)
            })
        {
            return false;
        }
        if !self.spec_ids.is_empty()
            && !self
                .spec_ids
                .iter()
                .any(|spec_id| normalize_spec_id(spec_id) == normalize_spec_id(&case.spec_id))
        {
            return false;
        }
        if !self.spec_nums.is_empty()
            && !self
                .spec_nums
                .iter()
                .any(|spec_num| spec_num == &case.spec_num)
        {
            return false;
        }
        if !self.conformance.is_empty() && !self.conformance.contains(&case.conformance) {
            return false;
        }
        if !self.datamodels.is_empty()
            && !self
                .datamodels
                .iter()
                .any(|datamodel| datamodel == &case.datamodel)
        {
            return false;
        }
        if !self.tags.is_empty()
            && !self
                .tags
                .iter()
                .all(|tag| case.tags.iter().any(|case_tag| case_tag == tag))
        {
            return false;
        }
        true
    }
}

#[must_use]
/// Parses the leading ASCII-decimal portion of a case identifier.
///
/// Returns `None` when `id` has no leading digits or the value exceeds `u64`.
pub fn numeric_case_id(id: &str) -> Option<u64> {
    let prefix = id.bytes().take_while(u8::is_ascii_digit).count();
    (prefix > 0).then(|| id[..prefix].parse().ok()).flatten()
}

fn normalize_spec_id(value: &str) -> &str {
    value.strip_prefix('#').unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_select_cases_only_and_reject_a_disposition_selector() {
        // A disposition is known only after a case runs, so it can never
        // select one; the filter identity has no such field.
        let decoded = serde_json::from_value::<Filters>(serde_json::json!({
            "dispositions": ["passed"]
        }));
        assert!(decoded.is_err(), "{decoded:?}");
        assert_eq!(
            serde_json::from_value::<Filters>(serde_json::json!({})).ok(),
            Some(Filters::default())
        );
    }
}
