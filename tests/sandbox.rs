//! End-to-end tests against real components built from `plugins/`.
//! The first test to run compiles the plugins for wasm32-wasip2.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use plugin_runtime::{Error, Manifest, Outcome, Runtime};

fn plugin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
        let status = Command::new(env!("CARGO"))
            .args(["build", "--release", "--target", "wasm32-wasip2", "--quiet"])
            .current_dir(&root)
            .status()
            .expect("run cargo for plugins");
        assert!(
            status.success(),
            "building plugins failed; is the wasm32-wasip2 target installed?"
        );
        root.join("target/wasm32-wasip2/release")
    })
}

/// Build a manifest for `wasm` (file stem) with extra TOML appended.
fn manifest(name: &str, wasm: &str, extra: &str) -> Manifest {
    let path = plugin_dir().join(format!("{wasm}.wasm"));
    let text = format!(
        "name = {name:?}\ncomponent = {:?}\n{extra}",
        path.display().to_string()
    );
    Manifest::parse(&text, Path::new(".")).expect("valid manifest")
}

fn runtime_with(ms: Vec<Manifest>) -> Runtime {
    let mut rt = Runtime::new().unwrap();
    for m in ms {
        rt.load(m).unwrap();
    }
    rt
}

const TIGHT: &str = "[limits]\nfuel = 200_000_000\nmemory-mib = 32\ntimeout-ms = 2000\nmax-log-bytes = 1024\nmax-output-bytes = 65536\n";

#[test]
fn hello_runs_and_logs() {
    let rt = runtime_with(vec![manifest("hello", "hello", "")]);
    let r = rt.call("hello", "world").unwrap();
    assert_eq!(r.outcome, Outcome::Ok("hello, world".into()));
    assert_eq!(r.logs.len(), 1);
    assert_eq!(r.logs[0].level, "info");
    assert!(r.fuel_used > 0);

    let info = rt.describe("hello").unwrap();
    assert_eq!(info.name, "hello");
}

#[test]
fn plugin_error_is_not_a_trap() {
    let rt = runtime_with(vec![manifest("hello", "hello", "")]);
    let r = rt.call("hello", "  ").unwrap();
    assert_eq!(
        r.outcome,
        Outcome::PluginError("input must not be empty".into())
    );
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    let r = rt.call("hostile", "spin").unwrap();
    assert_eq!(r.outcome, Outcome::FuelExhausted);
    assert_eq!(r.fuel_used, 200_000_000);
}

#[test]
fn infinite_loop_hits_wall_clock_when_fuel_is_generous() {
    let extra = "[limits]\nfuel = 100_000_000_000\ntimeout-ms = 100\n";
    let rt = runtime_with(vec![manifest("hostile", "hostile", extra)]);
    let r = rt.call("hostile", "spin").unwrap();
    assert_eq!(r.outcome, Outcome::Timeout);
    assert!(
        r.elapsed >= Duration::from_millis(100),
        "stopped early: {:?}",
        r.elapsed
    );
    assert!(
        r.elapsed < Duration::from_millis(1000),
        "stopped late: {:?}",
        r.elapsed
    );
}

#[test]
fn memory_growth_is_capped() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    let r = rt.call("hostile", "alloc").unwrap();
    assert_eq!(r.outcome, Outcome::MemoryLimit);
    assert!(
        r.stderr.contains("memory allocation"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn panic_is_a_trap_with_message_captured() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    let r = rt.call("hostile", "panic").unwrap();
    assert!(matches!(r.outcome, Outcome::Trap(_)), "{:?}", r.outcome);
    assert!(
        r.stderr.contains("plugin panicked on purpose"),
        "stderr: {}",
        r.stderr
    );
}

#[test]
fn log_flood_is_bounded() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    let r = rt.call("hostile", "log-flood").unwrap();
    let kept: usize = r.logs.iter().map(|l| l.message.len()).sum();
    assert!(kept <= 1024);
    assert!(r.logs_dropped > 90_000, "dropped {}", r.logs_dropped);
}

#[test]
fn oversized_output_is_rejected() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    let r = rt.call("hostile", "big-output").unwrap();
    assert_eq!(
        r.outcome,
        Outcome::OutputTooLarge {
            bytes: 8 << 20,
            limit: 65536
        }
    );
}

#[test]
fn no_ambient_authority() {
    let rt = runtime_with(vec![manifest("hostile", "hostile", TIGHT)]);
    // KV not granted: host returns an error instead of data.
    let r = rt.call("hostile", "kv").unwrap();
    assert_eq!(
        r.outcome,
        Outcome::PluginError("capability kv not granted".into())
    );
    // No preopens: the host filesystem does not exist for the plugin.
    let r = rt.call("hostile", "fs").unwrap();
    assert!(
        matches!(r.outcome, Outcome::PluginError(_)),
        "{:?}",
        r.outcome
    );
    // No environment variables leak in.
    let r = rt.call("hostile", "env").unwrap();
    assert_eq!(r.outcome, Outcome::Ok(String::new()));
}

#[test]
fn host_survives_hostile_calls() {
    let rt = runtime_with(vec![
        manifest("hostile", "hostile", TIGHT),
        manifest("hello", "hello", ""),
    ]);
    for mode in ["spin", "alloc", "panic", "big-output"] {
        assert!(!rt.call("hostile", mode).unwrap().outcome.is_ok());
        assert_eq!(
            rt.call("hello", "again").unwrap().outcome,
            Outcome::Ok("hello, again".into())
        );
    }
}

#[test]
fn kv_persists_across_calls_and_is_namespaced() {
    let caps = "[capabilities]\nkv = true\n";
    let rt = runtime_with(vec![
        manifest("counter-a", "kv_counter", caps),
        manifest("counter-b", "kv_counter", caps),
    ]);
    for want in ["1", "2", "3"] {
        assert_eq!(
            rt.call("counter-a", "hits").unwrap().outcome,
            Outcome::Ok(want.into())
        );
    }
    // Same component, different plugin name: separate namespace.
    assert_eq!(
        rt.call("counter-b", "hits").unwrap().outcome,
        Outcome::Ok("1".into())
    );
}

#[test]
fn filesystem_grant_is_read_only_and_scoped() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("note.txt"), "inside").unwrap();
    let extra = format!(
        "[capabilities]\nread-dirs = [{{ host = {:?}, guest = \"/data\" }}]\n",
        dir.path().display().to_string()
    );
    let rt = runtime_with(vec![manifest("fs-cat", "fs_cat", &extra)]);

    assert_eq!(
        rt.call("fs-cat", "/data/note.txt").unwrap().outcome,
        Outcome::Ok("inside".into())
    );
    for escape in ["/data/../etc/passwd", "/etc/passwd", "../note.txt"] {
        let r = rt.call("fs-cat", escape).unwrap();
        assert!(
            matches!(r.outcome, Outcome::PluginError(_)),
            "{escape}: {:?}",
            r.outcome
        );
    }
    let r = rt.call("fs-cat", "write:/data/new.txt").unwrap();
    assert!(
        matches!(r.outcome, Outcome::PluginError(_)),
        "write allowed: {:?}",
        r.outcome
    );
    assert!(!dir.path().join("new.txt").exists());
}

#[test]
fn concurrent_calls_are_isolated() {
    let rt = std::sync::Arc::new(runtime_with(vec![
        manifest("hello", "hello", ""),
        manifest("hostile", "hostile", TIGHT),
    ]));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let rt = rt.clone();
            std::thread::spawn(move || {
                if i % 2 == 0 {
                    for _ in 0..20 {
                        let r = rt.call("hello", &format!("t{i}")).unwrap();
                        assert_eq!(r.outcome, Outcome::Ok(format!("hello, t{i}")));
                    }
                } else {
                    assert_eq!(
                        rt.call("hostile", "spin").unwrap().outcome,
                        Outcome::FuelExhausted
                    );
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}

#[test]
fn wordfreq_is_deterministic() {
    let rt = runtime_with(vec![manifest("wordfreq", "wordfreq", "")]);
    let r = rt.call("wordfreq", "b a B c a b").unwrap();
    assert_eq!(r.outcome, Outcome::Ok("b 3\na 2\nc 1".into()));
}

#[test]
fn errors_before_running() {
    let rt = runtime_with(vec![manifest("hello", "hello", "")]);
    assert!(matches!(rt.call("nope", "x"), Err(Error::UnknownPlugin(_))));
    let big = "x".repeat(plugin_runtime::MAX_INPUT_BYTES + 1);
    assert!(matches!(
        rt.call("hello", &big),
        Err(Error::InputTooLarge(_))
    ));

    let mut rt = Runtime::new().unwrap();
    rt.load(manifest("hello", "hello", "")).unwrap();
    assert!(matches!(
        rt.load(manifest("hello", "hello", "")),
        Err(Error::Duplicate(_))
    ));
    let missing = Manifest::parse(
        "name = \"ghost\"\ncomponent = \"/nonexistent.wasm\"\n",
        Path::new("."),
    )
    .unwrap();
    assert!(matches!(rt.load(missing), Err(Error::Load { .. })));
}
