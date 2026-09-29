//! Dictionary parity gate: zh-CN and en must expose identical key sets.
//!
//! The i18n fallback chain (`current → zh-CN → raw key`) only holds when
//! both dictionaries share their key set: with lazy per-locale loading the
//! fallback dictionary is never loaded at runtime, so a key added to one
//! dictionary but not the other surfaces as a raw i18n key for users of the
//! other locale — and only there. Frontend has no test infra yet; this
//! integration test rides the existing `cargo test` CI leg as a cheap guard.

use serde_json::Value;
use std::collections::BTreeSet;

/// Recursively flatten nested JSON objects into dotted key paths,
/// mirroring how `t()` addresses entries ("domain.sub.key").
fn flatten(prefix: &str, value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten(&key, v, out);
            }
        }
        _ => {
            out.insert(prefix.to_string());
        }
    }
}

fn dictionary_keys(file: &str) -> BTreeSet<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../src/lib/i18n/dictionaries/"
    );
    let raw = std::fs::read_to_string(format!("{path}{file}"))
        .unwrap_or_else(|e| panic!("cannot read {file}: {e}"));
    let value: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("invalid JSON in {file}: {e}"));
    let mut out = BTreeSet::new();
    flatten("", &value, &mut out);
    out
}

#[test]
fn zh_cn_and_en_dictionaries_have_identical_key_sets() {
    let zh = dictionary_keys("zh-CN.json");
    let en = dictionary_keys("en.json");
    let zh_only: Vec<_> = zh.difference(&en).collect();
    let en_only: Vec<_> = en.difference(&zh).collect();
    assert!(
        zh_only.is_empty() && en_only.is_empty(),
        "dictionary key mismatch (missing keys render as raw i18n keys):\n  \
         zh-only: {zh_only:?}\n  en-only: {en_only:?}"
    );
}
