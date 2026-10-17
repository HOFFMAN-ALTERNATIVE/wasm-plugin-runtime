//! `wpr`: run, inspect and benchmark plugins from the command line.
//!
//! Exit codes: 0 call succeeded, 1 plugin failed or hit a limit, 2 bad usage,
//! manifest or component.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use plugin_runtime::{Manifest, Outcome, Report, Runtime};
use serde_json::json;

#[derive(Parser)]
#[command(version, about = "Sandboxed WebAssembly component plugin host")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a plugin once (or --repeat times) and print a JSON report per call.
    Run {
        manifest: PathBuf,
        /// Input string. Reads stdin when omitted.
        #[arg(long)]
        input: Option<String>,
        /// Run the same input N times against one runtime (KV persists in-process).
        #[arg(long, default_value_t = 1)]
        repeat: u32,
    },
    /// Validate the manifest, compile the component and print its metadata.
    Inspect { manifest: PathBuf },
    /// Measure per-call latency (fresh instance each call) and throughput.
    Bench {
        manifest: PathBuf,
        #[arg(long, default_value = "bench")]
        input: String,
        #[arg(long, default_value_t = 2000)]
        iterations: u32,
        #[arg(long, default_value_t = 1)]
        threads: u32,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("wpr: {e}");
            ExitCode::from(2)
        }
    }
}

fn load(path: &PathBuf) -> Result<(Runtime, String), Box<dyn std::error::Error>> {
    let manifest = Manifest::load(path)?;
    let name = manifest.name.clone();
    let mut rt = Runtime::new()?;
    rt.load(manifest)?;
    Ok((rt, name))
}

fn run(cli: Cli) -> Result<ExitCode, Box<dyn std::error::Error>> {
    match cli.cmd {
        Cmd::Run {
            manifest,
            input,
            repeat,
        } => {
            let input = match input {
                Some(s) => s,
                None => {
                    let mut s = String::new();
                    std::io::stdin()
                        .take(plugin_runtime::MAX_INPUT_BYTES as u64 + 1)
                        .read_to_string(&mut s)?;
                    s
                }
            };
            let (rt, name) = load(&manifest)?;
            let mut all_ok = true;
            for _ in 0..repeat.max(1) {
                let report = rt.call(&name, &input)?;
                all_ok &= report.outcome.is_ok();
                println!("{}", report_json(&name, &report));
            }
            Ok(if all_ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            })
        }
        Cmd::Inspect { manifest } => {
            let started = Instant::now();
            let (rt, name) = load(&manifest)?;
            let compile_ms = started.elapsed().as_secs_f64() * 1e3;
            let info = rt.describe(&name)?;
            let m = Manifest::load(&manifest)?;
            let out = json!({
                "manifest_name": name,
                "plugin": { "name": info.name, "version": info.version, "description": info.description },
                "limits": {
                    "fuel": m.limits.fuel,
                    "memory_mib": m.limits.memory_mib,
                    "timeout_ms": m.limits.timeout_ms,
                    "max_log_bytes": m.limits.max_log_bytes,
                    "max_output_bytes": m.limits.max_output_bytes,
                },
                "capabilities": {
                    "kv": m.capabilities.kv,
                    "read_dirs": m.capabilities.read_dirs.iter().map(|d| d.guest.clone()).collect::<Vec<_>>(),
                },
                "load_ms": (compile_ms * 10.0).round() / 10.0,
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Bench {
            manifest,
            input,
            iterations,
            threads,
        } => {
            let (rt, name) = load(&manifest)?;
            let rt = Arc::new(rt);
            let threads = threads.max(1);
            let per_thread = iterations.max(1).div_ceil(threads);
            // One untimed warm-up call so lazy initialisation is not measured.
            rt.call(&name, &input)?;
            let started = Instant::now();
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let rt = rt.clone();
                    let name = name.clone();
                    let input = input.clone();
                    std::thread::spawn(move || -> Result<Vec<Duration>, String> {
                        let mut lat = Vec::with_capacity(per_thread as usize);
                        for _ in 0..per_thread {
                            let r = rt.call(&name, &input).map_err(|e| e.to_string())?;
                            if !r.outcome.is_ok() {
                                return Err(format!("call failed: {}", r.outcome.kind()));
                            }
                            lat.push(r.elapsed);
                        }
                        Ok(lat)
                    })
                })
                .collect();
            let mut lat = Vec::new();
            for h in handles {
                lat.extend(h.join().map_err(|_| "bench thread panicked")??);
            }
            let wall = started.elapsed();
            lat.sort_unstable();
            let pct =
                |p: f64| lat[((lat.len() as f64 - 1.0) * p).round() as usize].as_secs_f64() * 1e6;
            println!("| plugin | calls | threads | calls/s | p50 µs | p99 µs | max µs |");
            println!("|---|---|---|---|---|---|---|");
            println!(
                "| {name} | {} | {threads} | {:.0} | {:.0} | {:.0} | {:.0} |",
                lat.len(),
                lat.len() as f64 / wall.as_secs_f64(),
                pct(0.50),
                pct(0.99),
                pct(1.0),
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn report_json(name: &str, r: &Report) -> serde_json::Value {
    let (value, error) = match &r.outcome {
        Outcome::Ok(v) => (Some(v.clone()), None),
        Outcome::PluginError(e) | Outcome::Trap(e) => (None, Some(e.clone())),
        Outcome::OutputTooLarge { bytes, limit } => {
            (None, Some(format!("{bytes} bytes > {limit}")))
        }
        Outcome::FuelExhausted | Outcome::Timeout | Outcome::MemoryLimit => (None, None),
    };
    json!({
        "plugin": name,
        "outcome": r.outcome.kind(),
        "output": value,
        "error": error,
        "fuel_used": r.fuel_used,
        "elapsed_us": r.elapsed.as_micros() as u64,
        "logs": r.logs.iter().map(|l| json!({"level": l.level, "message": l.message})).collect::<Vec<_>>(),
        "logs_dropped": r.logs_dropped,
        "stderr": if r.stderr.is_empty() { None } else { Some(&r.stderr) },
    })
}
