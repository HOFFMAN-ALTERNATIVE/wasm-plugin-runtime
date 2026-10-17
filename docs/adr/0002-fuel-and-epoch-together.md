# 0002. Fuel and epoch interruption together
Status: accepted
Date: 2026-10-17

## Context
An untrusted plugin can run forever. Wasmtime offers two ways to stop it. Fuel counts instructions and traps at zero; it is deterministic but costs some throughput and does not advance while the guest is blocked inside a host call. Epoch interruption checks a global counter at loop headers and function entries; it is cheap and maps to wall-clock time, but results vary with machine load.

## Options
1. Fuel only. Reproducible limits and exact accounting. A plugin stuck in a slow host import never times out.
2. Epoch only. Lowest overhead, real wall-clock bound. Not reproducible, and "how much work did this call do" has no answer.
3. Both, independently configured per manifest.

## Decision
Option 3. Fuel gives reproducible limits and a per-call cost figure for billing or quotas; the epoch deadline gives the latency guarantee a host actually promises its callers.

## Consequences
- Tests can assert exact fuel usage (`fuel_used == budget`) and a wall-clock window (100 ms timeout stops between 100 ms and 1 s).
- A background thread ticks the epoch every 5 ms, so timeouts round up to 5 ms.
- Fuel metering adds overhead to every call. If a deployment only needs wall-clock limits, making fuel optional per engine is a small change.
