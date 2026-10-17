//! Deliberately misbehaving plugin. Each input selects one attack; the host
//! test suite asserts that every one is contained without harming the host.
wit_bindgen::generate!({ path: "../../wit", world: "plugin" });

use wpr::plugin::host::{Level, kv_get, log};

struct Hostile;

impl Guest for Hostile {
    fn describe() -> Info {
        Info {
            name: "hostile".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Test fixture: spin, alloc, panic, log-flood, big-output, kv, fs".into(),
        }
    }

    fn run(input: String) -> Result<String, String> {
        match input.trim() {
            // Burns fuel, or wall-clock time if fuel is generous.
            "spin" => {
                let mut x: u64 = 0;
                loop {
                    x = std::hint::black_box(x.wrapping_add(1));
                }
            }
            // Grows linear memory 1 MiB at a time until the limiter says no.
            "alloc" => {
                let mut hoard: Vec<Vec<u8>> = Vec::new();
                loop {
                    hoard.push(vec![1u8; 1 << 20]);
                    std::hint::black_box(&hoard);
                }
            }
            "panic" => panic!("plugin panicked on purpose"),
            "log-flood" => {
                for i in 0..100_000 {
                    log(Level::Warn, &format!("flood line {i}"));
                }
                Ok("flooded".into())
            }
            "big-output" => Ok("x".repeat(8 << 20)),
            // Uses KV without the capability; the host must return an error.
            "kv" => kv_get("anything").map(|v| format!("{v:?}")),
            // Tries to read outside any preopen.
            "fs" => std::fs::read_to_string("/etc/passwd").map_err(|e| e.to_string()),
            "env" => Ok(std::env::vars()
                .map(|(k, _)| k)
                .collect::<Vec<_>>()
                .join(",")),
            other => Err(format!("unknown mode {other:?}")),
        }
    }
}

export!(Hostile);
