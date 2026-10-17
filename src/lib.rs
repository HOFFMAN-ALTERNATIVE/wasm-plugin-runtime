//! Sandboxed host for WebAssembly component plugins.
//!
//! A [`Runtime`] compiles each plugin once and then runs every call in a
//! fresh instance with four independent limits: fuel (instructions), linear
//! memory, wall-clock time (epoch interruption) and capabilities declared in
//! the plugin's [`Manifest`]. A plugin that hits any limit gets an
//! [`Outcome`] describing why; the host process and other plugins carry on.

pub mod manifest;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, ResourceLimiter, Store, Trap};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub use manifest::{Capabilities, DirGrant, Limits, Manifest};

mod bindings {
    wasmtime::component::bindgen!({ path: "wit", world: "plugin" });
}

use bindings::wpr::plugin::host::{self, Level};
use bindings::{Plugin, PluginPre};

/// Epoch tick. Timeouts are rounded up to a whole number of ticks, so the
/// effective precision of `timeout-ms` is about this value.
pub const EPOCH_TICK: Duration = Duration::from_millis(5);
/// Largest input accepted by [`Runtime::call`].
pub const MAX_INPUT_BYTES: usize = 4 << 20;
/// Bytes of plugin stderr kept per call (panic messages land here).
const STDERR_CAPTURE: usize = 4096;
/// Per-plugin KV quotas.
const KV_MAX_KEYS: usize = 1024;
const KV_MAX_KEY_BYTES: usize = 256;
const KV_MAX_VALUE_BYTES: usize = 64 * 1024;

/// Errors that stop a plugin from being loaded or called at all. Plugin
/// misbehaviour during a call is not an `Error`; it is an [`Outcome`].
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("manifest: {0}")]
    Manifest(String),
    #[error("loading component {path}: {reason}")]
    Load { path: String, reason: String },
    #[error("plugin {0:?} is already loaded")]
    Duplicate(String),
    #[error("unknown plugin {0:?}")]
    UnknownPlugin(String),
    #[error("input is {0} bytes; limit is {MAX_INPUT_BYTES}")]
    InputTooLarge(usize),
    #[error("host: {0}")]
    Host(String),
}

/// How a single call ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `run` returned `ok`.
    Ok(String),
    /// `run` returned `err`; the plugin handled the failure itself.
    PluginError(String),
    /// The call used up `limits.fuel`.
    FuelExhausted,
    /// The call ran longer than `limits.timeout-ms`.
    Timeout,
    /// The plugin tried to grow memory past `limits.memory-mib` and trapped.
    MemoryLimit,
    /// `run` returned more than `limits.max-output-bytes`.
    OutputTooLarge { bytes: usize, limit: usize },
    /// Any other trap: panic, unreachable, out-of-bounds access.
    Trap(String),
}

impl Outcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, Outcome::Ok(_))
    }

    /// Short stable label for logs and metrics.
    pub fn kind(&self) -> &'static str {
        match self {
            Outcome::Ok(_) => "ok",
            Outcome::PluginError(_) => "plugin_error",
            Outcome::FuelExhausted => "fuel_exhausted",
            Outcome::Timeout => "timeout",
            Outcome::MemoryLimit => "memory_limit",
            Outcome::OutputTooLarge { .. } => "output_too_large",
            Outcome::Trap(_) => "trap",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub level: &'static str,
    pub message: String,
}

/// Result of one call plus what it cost.
#[derive(Debug, Clone)]
pub struct Report {
    pub outcome: Outcome,
    pub fuel_used: u64,
    /// Instantiation plus execution.
    pub elapsed: Duration,
    pub logs: Vec<LogLine>,
    pub logs_dropped: u32,
    /// Plugin stderr, truncated to a few KiB. Rust panics print here.
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
}

/// Shared key/value store, namespaced by plugin name. In-memory only.
#[derive(Clone, Default)]
struct Kv(Arc<Mutex<HashMap<String, HashMap<String, String>>>>);

impl Kv {
    fn get(&self, ns: &str, key: &str) -> Option<String> {
        let map = self.0.lock().unwrap_or_else(|p| p.into_inner());
        map.get(ns).and_then(|m| m.get(key)).cloned()
    }

    fn set(&self, ns: &str, key: String, value: String) -> Result<(), String> {
        if key.is_empty() || key.len() > KV_MAX_KEY_BYTES {
            return Err(format!("key must be 1..={KV_MAX_KEY_BYTES} bytes"));
        }
        if value.len() > KV_MAX_VALUE_BYTES {
            return Err(format!("value must be <= {KV_MAX_VALUE_BYTES} bytes"));
        }
        let mut map = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let ns_map = map.entry(ns.to_string()).or_default();
        if !ns_map.contains_key(&key) && ns_map.len() >= KV_MAX_KEYS {
            return Err(format!("namespace is full ({KV_MAX_KEYS} keys)"));
        }
        ns_map.insert(key, value);
        Ok(())
    }
}

/// Caps linear memory and records whether the cap was the reason a grow failed,
/// so the resulting trap can be reported as [`Outcome::MemoryLimit`].
struct MemoryCap {
    max_bytes: usize,
    denied: bool,
}

impl ResourceLimiter for MemoryCap {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _max: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if desired > self.max_bytes {
            self.denied = true;
            return Ok(false);
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _max: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= 100_000)
    }
}

/// Per-call store data.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    memory: MemoryCap,
    plugin: Arc<str>,
    kv_granted: bool,
    kv: Kv,
    logs: Vec<LogLine>,
    log_bytes: usize,
    log_budget: usize,
    logs_dropped: u32,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl host::Host for State {
    fn log(&mut self, level: Level, message: String) {
        if self.log_bytes + message.len() > self.log_budget {
            self.logs_dropped = self.logs_dropped.saturating_add(1);
            return;
        }
        self.log_bytes += message.len();
        let level = match level {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };
        self.logs.push(LogLine { level, message });
    }

    fn kv_get(&mut self, key: String) -> Result<Option<String>, String> {
        if !self.kv_granted {
            return Err("capability kv not granted".into());
        }
        Ok(self.kv.get(&self.plugin, &key))
    }

    fn kv_set(&mut self, key: String, value: String) -> Result<(), String> {
        if !self.kv_granted {
            return Err("capability kv not granted".into());
        }
        self.kv.set(&self.plugin, key, value)
    }
}

struct Loaded {
    manifest: Manifest,
    name: Arc<str>,
    pre: PluginPre<State>,
}

/// Background thread that advances the engine epoch every [`EPOCH_TICK`].
struct Ticker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Ticker {
    fn start(engine: Engine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name("wpr-epoch".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(EPOCH_TICK);
                    engine.increment_epoch();
                }
            })
            .expect("spawn epoch thread");
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Plugin host. Load plugins once with [`Runtime::load`], then call them from
/// any number of threads; each call gets its own store and instance.
pub struct Runtime {
    engine: Engine,
    linker: Linker<State>,
    kv: Kv,
    plugins: HashMap<String, Loaded>,
    _ticker: Ticker,
}

impl Runtime {
    pub fn new() -> Result<Self, Error> {
        let mut config = Config::new();
        config.consume_fuel(true);
        config.epoch_interruption(true);
        config.wasm_component_model(true);
        let engine = Engine::new(&config).map_err(|e| Error::Host(format!("{e:#}")))?;

        let mut linker = Linker::new(&engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|e| Error::Host(format!("{e:#}")))?;
        Plugin::add_to_linker::<_, HasSelf<State>>(&mut linker, |s| s)
            .map_err(|e| Error::Host(format!("{e:#}")))?;

        Ok(Self {
            _ticker: Ticker::start(engine.clone()),
            engine,
            linker,
            kv: Kv::default(),
            plugins: HashMap::new(),
        })
    }

    /// Compile the manifest's component and register it under `manifest.name`.
    /// Compilation happens once here; calls only instantiate.
    pub fn load(&mut self, manifest: Manifest) -> Result<(), Error> {
        if self.plugins.contains_key(&manifest.name) {
            return Err(Error::Duplicate(manifest.name));
        }
        let load_err = |e: wasmtime::Error| Error::Load {
            path: manifest.component.display().to_string(),
            reason: format!("{e:#}"),
        };
        let component =
            Component::from_file(&self.engine, &manifest.component).map_err(load_err)?;
        let instance_pre = self.linker.instantiate_pre(&component).map_err(load_err)?;
        let pre = PluginPre::new(instance_pre).map_err(load_err)?;
        let name: Arc<str> = Arc::from(manifest.name.as_str());
        self.plugins.insert(
            manifest.name.clone(),
            Loaded {
                manifest,
                name,
                pre,
            },
        );
        Ok(())
    }

    /// Names of loaded plugins, sorted.
    pub fn plugins(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.plugins.keys().map(String::as_str).collect();
        v.sort_unstable();
        v
    }

    /// Call the plugin's `describe` export under its normal limits.
    pub fn describe(&self, plugin: &str) -> Result<PluginInfo, Error> {
        let loaded = self.get(plugin)?;
        let (mut store, _stderr) = self.new_store(loaded)?;
        loaded
            .pre
            .instantiate(&mut store)
            .and_then(|p| p.call_describe(&mut store))
            .map(|i| PluginInfo {
                name: i.name,
                version: i.version,
                description: i.description,
            })
            .map_err(|e| Error::Host(format!("describe failed: {e:#}")))
    }

    /// Run one call of `plugin` with `input`. Limits are enforced inside the
    /// call; whatever the plugin does, this returns a [`Report`].
    pub fn call(&self, plugin: &str, input: &str) -> Result<Report, Error> {
        if input.len() > MAX_INPUT_BYTES {
            return Err(Error::InputTooLarge(input.len()));
        }
        let loaded = self.get(plugin)?;
        let limits = &loaded.manifest.limits;
        let started = Instant::now();
        let (mut store, stderr) = self.new_store(loaded)?;

        let result = loaded
            .pre
            .instantiate(&mut store)
            .and_then(|p| p.call_run(&mut store, input));
        let elapsed = started.elapsed();
        let fuel_used = limits.fuel - store.get_fuel().unwrap_or(0);
        let memory_denied = store.data().memory.denied;

        let outcome = match result {
            Ok(Ok(out)) if out.len() > limits.max_output_bytes => Outcome::OutputTooLarge {
                bytes: out.len(),
                limit: limits.max_output_bytes,
            },
            Ok(Ok(out)) => Outcome::Ok(out),
            Ok(Err(msg)) => Outcome::PluginError(msg),
            Err(e) => classify(&e, memory_denied),
        };

        let data = store.into_data();
        Ok(Report {
            outcome,
            fuel_used,
            elapsed,
            logs: data.logs,
            logs_dropped: data.logs_dropped,
            stderr: String::from_utf8_lossy(&stderr.contents()).into_owned(),
        })
    }

    fn get(&self, plugin: &str) -> Result<&Loaded, Error> {
        self.plugins
            .get(plugin)
            .ok_or_else(|| Error::UnknownPlugin(plugin.to_string()))
    }

    fn new_store(&self, loaded: &Loaded) -> Result<(Store<State>, MemoryOutputPipe), Error> {
        let m = &loaded.manifest;
        let stderr = MemoryOutputPipe::new(STDERR_CAPTURE);
        let mut wasi = WasiCtxBuilder::new();
        // No network, no env, no args, no inherited stdio. The only ambient
        // authority a plugin gets is clocks and randomness.
        wasi.allow_tcp(false)
            .allow_udp(false)
            .allow_ip_name_lookup(false)
            .socket_addr_check(|_, _| Box::pin(async { false }))
            .stderr(stderr.clone());
        for d in &m.capabilities.read_dirs {
            wasi.preopened_dir(&d.host, &d.guest, FsPerms::ReadOnly)
                .map_err(|e| Error::Host(format!("preopen {}: {e:#}", d.host.display())))?;
        }

        let state = State {
            wasi: wasi.build(),
            table: ResourceTable::new(),
            memory: MemoryCap {
                max_bytes: (m.limits.memory_mib as usize) << 20,
                denied: false,
            },
            plugin: loaded.name.clone(),
            kv_granted: m.capabilities.kv,
            kv: self.kv.clone(),
            logs: Vec::new(),
            log_bytes: 0,
            log_budget: m.limits.max_log_bytes,
            logs_dropped: 0,
        };
        let mut store = Store::new(&self.engine, state);
        store.limiter(|s| &mut s.memory);
        store
            .set_fuel(m.limits.fuel)
            .map_err(|e| Error::Host(format!("{e:#}")))?;
        let ticks = m.limits.timeout_ms.div_ceil(EPOCH_TICK.as_millis() as u64);
        store.set_epoch_deadline(ticks.max(1));
        store.epoch_deadline_trap();
        Ok((store, stderr))
    }
}

fn classify(err: &wasmtime::Error, memory_denied: bool) -> Outcome {
    match err.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => Outcome::FuelExhausted,
        Some(Trap::Interrupt) => Outcome::Timeout,
        _ if memory_denied => Outcome::MemoryLimit,
        Some(trap) => Outcome::Trap(trap.to_string()),
        None => Outcome::Trap(format!("{err:#}")),
    }
}
