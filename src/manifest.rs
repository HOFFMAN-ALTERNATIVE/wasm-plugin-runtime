//! Plugin manifest: what a plugin is allowed to do and how much it may spend.
//!
//! Parsing is strict (`deny_unknown_fields`) and every limit has a hard
//! ceiling, so a typo or an absurd value fails at load time instead of
//! silently granting more than intended.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;

/// Ceilings applied to every manifest. A host embedding the library can
/// still choose lower values; it cannot load a manifest that asks for more.
pub const MAX_MEMORY_MIB: u64 = 1024;
pub const MAX_TIMEOUT_MS: u64 = 60_000;
pub const MAX_FUEL: u64 = 100_000_000_000;
pub const MAX_LOG_BYTES: usize = 1 << 20;
pub const MAX_OUTPUT_BYTES: usize = 16 << 20;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Manifest {
    /// `[a-z0-9-]{1,64}`. Also the KV namespace.
    pub name: String,
    /// Path to the compiled component, relative to the manifest file.
    pub component: PathBuf,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub capabilities: Capabilities,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", default)]
pub struct Limits {
    /// Wasm instructions budget per call (wasmtime fuel units).
    pub fuel: u64,
    /// Linear memory cap per instance.
    pub memory_mib: u64,
    /// Wall-clock budget per call, enforced by epoch interruption.
    pub timeout_ms: u64,
    /// Total bytes of log messages kept per call; the rest are counted and dropped.
    pub max_log_bytes: usize,
    /// Largest `run` result accepted; larger results become an error.
    pub max_output_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: 500_000_000,
            memory_mib: 64,
            timeout_ms: 1_000,
            max_log_bytes: 16 * 1024,
            max_output_bytes: 1 << 20,
        }
    }
}

/// Everything is off unless the manifest turns it on.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case", default)]
pub struct Capabilities {
    /// Access to the per-plugin key/value store.
    pub kv: bool,
    /// Host directories exposed read-only to the plugin.
    pub read_dirs: Vec<DirGrant>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirGrant {
    /// Host path, relative to the manifest file.
    pub host: PathBuf,
    /// Absolute path the plugin sees, e.g. `/data`.
    pub guest: String,
}

impl Manifest {
    /// Read, parse and validate a manifest file. Relative paths inside it are
    /// resolved against the manifest's directory.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::Manifest(format!("{}: {e}", path.display())))?;
        let base = path.parent().unwrap_or(Path::new("."));
        Self::parse(&text, base)
    }

    /// Parse and validate manifest text; `base` resolves relative paths.
    pub fn parse(text: &str, base: &Path) -> Result<Self, Error> {
        let mut m: Manifest = toml::from_str(text).map_err(|e| Error::Manifest(e.to_string()))?;
        if m.component.is_relative() {
            m.component = base.join(&m.component);
        }
        for d in &mut m.capabilities.read_dirs {
            if d.host.is_relative() {
                d.host = base.join(&d.host);
            }
        }
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), Error> {
        let bad = |msg: String| Err(Error::Manifest(msg));
        let name_ok = !self.name.is_empty()
            && self.name.len() <= 64
            && self
                .name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !name_ok {
            return bad(format!("name {:?} must match [a-z0-9-]{{1,64}}", self.name));
        }
        let l = &self.limits;
        if l.fuel == 0 || l.fuel > MAX_FUEL {
            return bad(format!("limits.fuel must be 1..={MAX_FUEL}"));
        }
        if l.memory_mib == 0 || l.memory_mib > MAX_MEMORY_MIB {
            return bad(format!("limits.memory-mib must be 1..={MAX_MEMORY_MIB}"));
        }
        if l.timeout_ms == 0 || l.timeout_ms > MAX_TIMEOUT_MS {
            return bad(format!("limits.timeout-ms must be 1..={MAX_TIMEOUT_MS}"));
        }
        if l.max_log_bytes > MAX_LOG_BYTES {
            return bad(format!("limits.max-log-bytes must be <= {MAX_LOG_BYTES}"));
        }
        if l.max_output_bytes == 0 || l.max_output_bytes > MAX_OUTPUT_BYTES {
            return bad(format!(
                "limits.max-output-bytes must be 1..={MAX_OUTPUT_BYTES}"
            ));
        }
        for d in &self.capabilities.read_dirs {
            if !d.guest.starts_with('/') || d.guest.split('/').any(|seg| seg == "..") {
                return bad(format!(
                    "read-dirs guest path {:?} must be absolute without '..'",
                    d.guest
                ));
            }
            if !d.host.is_dir() {
                return bad(format!(
                    "read-dirs host path {} is not a directory",
                    d.host.display()
                ));
            }
        }
        Ok(())
    }
}
