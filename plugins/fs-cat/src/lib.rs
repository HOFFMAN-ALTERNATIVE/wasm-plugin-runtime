//! Reads a file through WASI. It can only see directories the manifest
//! preopens, and only read-only, so this is the filesystem capability test.
wit_bindgen::generate!({ path: "../../wit", world: "plugin" });

struct FsCat;

impl Guest for FsCat {
    fn describe() -> Info {
        Info {
            name: "fs-cat".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            description: "Returns the contents of a file under a preopened directory".into(),
        }
    }

    fn run(input: String) -> Result<String, String> {
        let path = input.trim();
        if let Some(rest) = path.strip_prefix("write:") {
            // Used by tests to prove preopens are read-only.
            return std::fs::write(rest, b"x")
                .map(|()| "written".to_string())
                .map_err(|e| format!("write {rest}: {e}"));
        }
        std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))
    }
}

export!(FsCat);
