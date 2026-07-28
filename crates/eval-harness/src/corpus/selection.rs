use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

use crate::error::EvalError;

#[derive(Debug, Clone)]
pub struct SampleRequest {
    pub corpus_fingerprint: String,
    pub per_stratum_count: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Clone)]
pub struct ShardSpec {
    pub index: u32,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub struct Selection {
    pub case_ids: Vec<String>,
    pub population_count: usize,
    pub selected_count: usize,
    pub fingerprint: String,
}

pub fn select_sample(
    case_ids: &[String],
    strata: &std::collections::BTreeMap<String, Vec<String>>,
    request: &SampleRequest,
) -> Result<Selection, EvalError> {
    if case_ids.is_empty() {
        return Err(EvalError::InvalidInput("case_ids must not be empty".into()));
    }

    let mut selected = BTreeSet::new();

    for (stratum, count) in &request.per_stratum_count {
        let stratum_ids = strata
            .get(stratum)
            .ok_or_else(|| EvalError::InvalidConfig(format!("missing stratum: {stratum}")))?;

        if *count > stratum_ids.len() {
            return Err(EvalError::InvalidConfig(format!(
                "requested {count} from stratum '{stratum}' but only {} available",
                stratum_ids.len()
            )));
        }

        let mut scored: Vec<(String, [u8; 32])> = stratum_ids
            .iter()
            .map(|id| {
                let mut hasher = Sha256::new();
                hasher.update(request.corpus_fingerprint.as_bytes());
                hasher.update(b"\0");
                hasher.update(id.as_bytes());
                let hash: [u8; 32] = hasher.finalize().into();
                (id.clone(), hash)
            })
            .collect();

        scored.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

        for (id, _) in scored.into_iter().take(*count) {
            selected.insert(id);
        }
    }

    let case_id_set: BTreeSet<&str> = case_ids.iter().map(String::as_str).collect();
    let invalid: Vec<&str> = selected
        .iter()
        .filter(|id| !case_id_set.contains(id.as_str()))
        .map(String::as_str)
        .collect();
    if !invalid.is_empty() {
        return Err(EvalError::InvalidInput(format!(
            "selected IDs not in case_ids: {invalid:?}"
        )));
    }

    let mut result: Vec<String> = selected.into_iter().collect();
    result.sort();

    let mut fingerprint_hasher = Sha256::new();
    fingerprint_hasher.update(request.corpus_fingerprint.as_bytes());
    for id in &result {
        fingerprint_hasher.update(id.as_bytes());
    }
    let fingerprint = hex::encode(fingerprint_hasher.finalize());

    Ok(Selection {
        population_count: case_ids.len(),
        selected_count: result.len(),
        case_ids: result,
        fingerprint,
    })
}

pub fn select_shard(
    case_ids: &[String],
    corpus_fingerprint: &str,
    shard: &ShardSpec,
) -> Result<Selection, EvalError> {
    if shard.count == 0 {
        return Err(EvalError::InvalidConfig(
            "shard count must be greater than zero".into(),
        ));
    }
    if shard.index >= shard.count {
        return Err(EvalError::InvalidConfig(format!(
            "shard index {} must be less than count {}",
            shard.index, shard.count
        )));
    }

    let selected: Vec<String> = case_ids
        .iter()
        .filter(|id| {
            let mut hasher = Sha256::new();
            hasher.update(corpus_fingerprint.as_bytes());
            hasher.update(b"\0");
            hasher.update(id.as_bytes());
            let hash: [u8; 8] = hasher.finalize()[..8].try_into().unwrap();
            let shard_index = u64::from_be_bytes(hash) % shard.count as u64;
            shard_index == shard.index as u64
        })
        .cloned()
        .collect();

    let mut fingerprint_hasher = Sha256::new();
    fingerprint_hasher.update(corpus_fingerprint.as_bytes());
    fingerprint_hasher.update(shard.index.to_string().as_bytes());
    fingerprint_hasher.update(shard.count.to_string().as_bytes());
    for id in &selected {
        fingerprint_hasher.update(id.as_bytes());
    }
    let fingerprint = hex::encode(fingerprint_hasher.finalize());

    Ok(Selection {
        population_count: case_ids.len(),
        selected_count: selected.len(),
        case_ids: selected,
        fingerprint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_ids(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("case-{i:03}")).collect()
    }

    fn sample_strata(ids: &[String]) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut strata: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for id in ids {
            let stratum = if id.ends_with('0') || id.ends_with('5') {
                "even"
            } else {
                "odd"
            }
            .to_string();
            strata.entry(stratum).or_default().push(id.clone());
        }
        strata
    }

    #[test]
    fn sample_is_independent_of_input_order() {
        let ids = sample_ids(100);
        let strata = sample_strata(&ids);
        let request = SampleRequest {
            corpus_fingerprint: "fp1".into(),
            per_stratum_count: [("even".into(), 5), ("odd".into(), 5)]
                .into_iter()
                .collect(),
        };

        let first = select_sample(&ids, &strata, &request).unwrap();
        let mut reversed = ids.clone();
        reversed.reverse();
        let second = select_sample(&reversed, &strata, &request).unwrap();

        assert_eq!(first.case_ids, second.case_ids);
    }

    #[test]
    fn shard_union_covers_all_cases() {
        let ids = sample_ids(50);
        let fp = "test-fingerprint";

        for shard_count in 1..=8u32 {
            let mut all_selected = BTreeSet::new();
            for idx in 0..shard_count {
                let selection = select_shard(
                    &ids,
                    fp,
                    &ShardSpec {
                        index: idx,
                        count: shard_count,
                    },
                )
                .unwrap();
                for id in &selection.case_ids {
                    assert!(
                        all_selected.insert(id.clone()),
                        "duplicate {id} across shards for count={shard_count}"
                    );
                }
            }
            assert_eq!(
                all_selected.len(),
                ids.len(),
                "shard union should cover all cases for count={shard_count}"
            );
        }
    }

    #[test]
    fn shard_rejects_zero_count() {
        let ids = sample_ids(10);
        assert!(select_shard(&ids, "fp", &ShardSpec { index: 0, count: 0 }).is_err());
    }

    #[test]
    fn shard_rejects_index_out_of_bounds() {
        let ids = sample_ids(10);
        assert!(select_shard(&ids, "fp", &ShardSpec { index: 5, count: 4 }).is_err());
    }

    #[test]
    fn sample_rejects_empty_case_ids() {
        let request = SampleRequest {
            corpus_fingerprint: "fp".into(),
            per_stratum_count: [("s".into(), 1)].into_iter().collect(),
        };
        assert!(select_sample(&[], &std::collections::BTreeMap::new(), &request).is_err());
    }

    #[test]
    fn sample_rejects_missing_stratum() {
        let ids = sample_ids(10);
        let empty_strata = std::collections::BTreeMap::new();
        let request = SampleRequest {
            corpus_fingerprint: "fp".into(),
            per_stratum_count: [("missing".into(), 1)].into_iter().collect(),
        };
        assert!(select_sample(&ids, &empty_strata, &request).is_err());
    }
}
