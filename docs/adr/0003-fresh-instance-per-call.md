# 0003. Fresh instance per call
Status: accepted
Date: 2026-10-17

## Context
Reusing a plugin instance across calls is faster, but any state the plugin keeps in linear memory then leaks between calls. If calls come from different users, that is a data leak. A trapped instance is also unusable afterwards, so reuse needs poisoning and recovery logic.

## Options
1. Long-lived instance per plugin. Fastest. Shared mutable state across callers; recovery after traps; memory only grows.
2. Instance pool with reset. Middle ground. Resetting memory correctly is subtle and easy to get wrong.
3. Fresh instance per call from a cached `InstancePre`. Compilation and import resolution happen once at load; each call only instantiates.

## Decision
Option 3. Isolation between calls is the property the project exists to provide, and measured cost is about 21 µs p50 per call for a small plugin.

## Consequences
- Plugins are stateless by construction. Durable state goes through the `kv` host import, which is namespaced and quota-limited.
- A trap never poisons later calls.
- Per-call memory map/unmap limits multi-thread scaling (30k calls/s on 4 threads vs 27.7k on 1). Next step is wasmtime's pooling allocator, which keeps isolation and reuses pre-mapped slots.
