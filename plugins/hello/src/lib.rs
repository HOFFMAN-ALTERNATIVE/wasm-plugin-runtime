//! Smallest useful plugin: echoes a greeting and writes one log line.
wit_bindgen::generate!({ path: "../../wit", world: "plugin" });

use wpr::plugin::host::{Level, log};

struct Hello;

impl Guest for Hello {
    fn describe() -> Info {
        Info {
            name: "hello".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Returns a greeting for the input".into(),
        }
    }

    fn run(input: String) -> Result<String, String> {
        let who = input.trim();
        if who.is_empty() {
            return Err("input must not be empty".into());
        }
        log(
            Level::Info,
            &format!("greeting {} bytes of input", who.len()),
        );
        Ok(format!("hello, {who}"))
    }
}

export!(Hello);
