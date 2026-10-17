use std::path::Path;

use plugin_runtime::Manifest;

fn parse(text: &str) -> Result<Manifest, String> {
    Manifest::parse(text, Path::new("/srv/plugins")).map_err(|e| e.to_string())
}

#[test]
fn defaults_and_relative_paths() {
    let m = parse("name = \"ok-1\"\ncomponent = \"x.wasm\"\n").unwrap();
    assert_eq!(m.component, Path::new("/srv/plugins/x.wasm"));
    assert_eq!(m.limits.memory_mib, 64);
    assert!(!m.capabilities.kv);
    assert!(m.capabilities.read_dirs.is_empty());
}

#[test]
fn rejects_unknown_fields() {
    // A typo must not silently fall back to a default.
    let e = parse("name = \"a\"\ncomponent = \"x.wasm\"\n[limits]\nmemory_mb = 10\n").unwrap_err();
    assert!(e.contains("unknown field"), "{e}");
    let e =
        parse("name = \"a\"\ncomponent = \"x.wasm\"\n[capabilities]\nnet = true\n").unwrap_err();
    assert!(e.contains("unknown field"), "{e}");
}

#[test]
fn rejects_bad_names() {
    for name in ["", "Upper", "has space", "../x", &"a".repeat(65)] {
        let e = parse(&format!("name = {name:?}\ncomponent = \"x.wasm\"\n")).unwrap_err();
        assert!(e.contains("name"), "{name}: {e}");
    }
}

#[test]
fn rejects_out_of_range_limits() {
    for limits in [
        "fuel = 0",
        "memory-mib = 0",
        "memory-mib = 4096",
        "timeout-ms = 0",
        "timeout-ms = 600000",
        "max-output-bytes = 0",
        "max-log-bytes = 10000000",
    ] {
        let text = format!("name = \"a\"\ncomponent = \"x.wasm\"\n[limits]\n{limits}\n");
        assert!(parse(&text).is_err(), "{limits} accepted");
    }
}

#[test]
fn rejects_bad_dir_grants() {
    let dir = tempfile::tempdir().unwrap();
    let host = dir.path().display().to_string();
    for guest in ["data", "/data/../etc"] {
        let text = format!(
            "name = \"a\"\ncomponent = \"x.wasm\"\n[capabilities]\nread-dirs = [{{ host = {host:?}, guest = {guest:?} }}]\n"
        );
        assert!(parse(&text).is_err(), "{guest} accepted");
    }
    let text = "name = \"a\"\ncomponent = \"x.wasm\"\n[capabilities]\nread-dirs = [{ host = \"/does/not/exist\", guest = \"/d\" }]\n";
    assert!(parse(text).is_err());
}
