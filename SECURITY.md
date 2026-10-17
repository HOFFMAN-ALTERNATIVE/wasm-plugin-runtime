# Security

## Reporting

Please report vulnerabilities privately through GitHub's "Report a vulnerability" button on this repository (Security tab). I aim to reply within 72 hours. Do not open a public issue for anything exploitable.

## Threat model

- **Asset:** the host process and machine: memory, CPU time, files, network, environment, other plugins' data.
- **Actor:** a plugin author who ships a malicious or buggy component.
- **Entry points:** the component binary, the `run` input string, the three host imports (`log`, `kv-get`, `kv-set`), and WASI interfaces linked into every instance.
- **Top risks and mitigations:**
  - CPU exhaustion: fuel per call plus epoch deadline (wall clock).
  - Memory exhaustion: `ResourceLimiter` caps linear memory per instance; output, input, log and stderr sizes are bounded.
  - Data access: no preopens unless granted, grants are read-only, no env, no args, no sockets (`allow_tcp/udp(false)`, address check denies all, no DNS).
  - Cross-plugin leakage: fresh instance per call; KV namespaced by manifest name with key/value/size quotas.
  - Config mistakes: strict manifest parsing (`deny_unknown_fields`) with hard ceilings on every limit.

## Out of scope

Bugs in wasmtime or Cranelift that break the WebAssembly sandbox itself. Those belong upstream at https://github.com/bytecodealliance/wasmtime/security. This project pins wasmtime in `Cargo.lock` and runs `cargo audit` in CI.

Manifests are trusted input: whoever writes the manifest decides what a plugin may do.
