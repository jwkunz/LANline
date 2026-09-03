//! Small helpers shared across handlers.

use serde_json::Value;

/// Recursively merge `patch` into `base` (RFC 7386-style: objects merge,
/// everything else replaces). `null` in `patch` overwrites with `null` rather
/// than deleting — the server has no "unset" semantics in phase 1.
pub fn merge_json(base: &mut Value, patch: &Value) {
    match (base, patch) {
        (Value::Object(base_map), Value::Object(patch_map)) => {
            for (k, v) in patch_map {
                merge_json(base_map.entry(k.clone()).or_insert(Value::Null), v);
            }
        }
        (base_slot, patch_val) => {
            *base_slot = patch_val.clone();
        }
    }
}
