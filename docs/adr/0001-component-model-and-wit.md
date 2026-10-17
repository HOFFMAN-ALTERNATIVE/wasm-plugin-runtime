# 0001. Component model and WIT instead of raw core modules
Status: accepted
Date: 2026-10-17

## Context
Plugins exchange strings and results with the host and call a small host API. With core wasm modules the only types are integers, so every string crossing the boundary needs a hand-written ABI: who allocates, who frees, how errors are encoded. That code is where plugin hosts usually get memory-safety bugs, and it ties plugins to one language's conventions.

## Options
1. Core modules with a custom ABI (pointer + length, host-exported alloc). Works on every runtime and toolchain. Every plugin language needs a hand-written shim, and bugs in that shim are host bugs.
2. Core modules with an existing framework (Extism-style). Mature, multi-language. Adds a large dependency and its own conventions for something a 34-line WIT file can describe.
3. Component model with WIT. Types, strings, `result` and `option` are generated on both sides by `wasmtime::component::bindgen!` and `wit-bindgen`. Requires a recent toolchain (`wasm32-wasip2`, wasmtime 49 needs Rust 1.96).

## Decision
Option 3. The canonical ABI removes the hand-written marshalling layer entirely, which is the riskiest part of a plugin host.

## Consequences
- The contract is `wit/plugin.wit`; changing it is a versioned, reviewable diff.
- Plugins in Rust, C, Go (TinyGo) or JS (componentize-js) can target it without host changes.
- MSRV follows wasmtime (currently 1.96). Older embedders cannot use this crate.
- Revisit if a target platform needs a runtime without component-model support.
