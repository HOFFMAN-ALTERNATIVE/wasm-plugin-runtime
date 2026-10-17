//! Stateful plugin: increments a counter in host KV storage.
//! Instances are fresh per call, so the only state that survives is in KV.
wit_bindgen::generate!({ path: "../../wit", world: "plugin" });

use wpr::plugin::host::{kv_get, kv_set};

struct Counter;

impl Guest for Counter {
    fn describe() -> Info {
        Info {
            name: "kv-counter".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Increments a named counter in host KV (needs capabilities.kv)".into(),
        }
    }

    fn run(input: String) -> Result<String, String> {
        let key = match input.trim() {
            "" => "count",
            k => k,
        };
        let current = match kv_get(key)? {
            Some(v) => v
                .parse::<u64>()
                .map_err(|e| format!("stored value is not a number: {e}"))?,
            None => 0,
        };
        let next = current + 1;
        kv_set(key, &next.to_string())?;
        Ok(next.to_string())
    }
}

export!(Counter);
