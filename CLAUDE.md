# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Read [AGENTS.md](AGENTS.md) first. It holds the project rules (the frozen Go reference, `GoStr` case folding, byte-for-byte passthrough, credential handling, the request-logging sink, linear-time body work, `ponytail:` comments, prompt-cache constraints, review priorities) and is not repeated here. If you are installing cliproxy-rs for a user rather than developing it, follow `docs/AI-SETUP.md` instead. Note that `CONTRIBUTING.md` still names the older reference commit `6fecc6e`; AGENTS.md and `docs/PARITY.md` are current (v8.0.35, `ae3ee3be`).

## Commands

Toolchain: Rust stable 1.99 or newer (CI pins 1.99.1), plus `cmake`, `clang` and `perl` for the BoringSSL build. On Windows also LLVM (`LIBCLANG_PATH`) and NASM on `PATH`; native plugins and their tests are Unix-only.

```sh
# The three CI gates, in order
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

# While iterating: one crate, or a name filter (the full suite is slow and memory-heavy)
cargo test -p cpa-exec kimi
cargo test -p cpa-server --test routes
cargo test -p cpa-translate --test golden claude_responses

# Faster full runs, as CI does (settings in .config/nextest.toml; cargo test ignores them)
cargo nextest run --workspace --locked --profile ci
cargo test --workspace --doc --locked                      # nextest skips doc tests

# Unit tests behind the optional server features
cargo test -p cpa-server --features media-relay,pprof --lib

# The patched vendor crate (outside the workspace; see vendor/README.md)
cargo test --locked --manifest-path vendor/gjson/Cargo.toml --target-dir target

# Run the proxy from source (Go-style single-dash flags work: -config, -home-jwt, -discover, -vertex-import)
cargo run -p cliproxy -- -config config.yaml

# Release binary and the Linux performance gates (bench/budgets.txt)
cargo build --release -p cliproxy
bench/gate.sh size x86_64-unknown-linux-gnu target/release/cliproxy
bench/gate.sh idle linux target/release/cliproxy 1

# Dashboard (ui/, Svelte 5, Node 22.12+). ui/dist is checked in and embedded by
# crates/cpa-server/build.rs, so rebuild it before compiling Rust after a UI change.
cd ui && npm ci && npm run check && npm test && npm run build

# Regenerate the parity audit (docs/parity-audit/status.md and the PARITY.md/README summaries)
python3 docs/parity-audit/audit.py
```

On a shared or small machine prefix builds with `CARGO_BUILD_JOBS=2 nice -n 19`. Set `CARGO_PROFILE_TEST_DEBUG=0` to keep the ~55 test binaries small.

### Test environment

- Tests that need Go 1.26 (plugin tests build cgo plugins from `crates/cpa-plugin/tests/goplugins`) or PostgreSQL (`cpa-store`, pointed at server binaries by `CPA_TEST_PG_BIN`) skip when the tool is missing. `CPA_TEST_NO_SKIP=1`, as in CI, turns those skips into failures.
- CI runs the suite inside a loopback-only network namespace. Locally, `cpa_server::testing::runtime` routes any non-loopback credential to a dead proxy so a test cannot reach a provider; mark a credential whose executor points at a mock with `cpa_server::testing::local`.
- Every test needs its own `auth-dir`. Without one, credential loading falls back to `~/.cli-proxy-api`, where real logins live.
- `.config/nextest.toml` serialises the heap gate (`crates/cpa-server/tests/alloc_budget.rs`), the wall-clock-bounded tests and the Go plugin tests. If a timing test flakes under `cargo test`, that is why.
- The Go/Rust differential harness (`./harness/run`, Linux only, needs Go and user namespaces) builds the unmodified Go reference and replays 57 fixtures through both. It has its own workspace in `harness/rust` so `cargo test --workspace` never needs Go; that workspace has its own `Cargo.lock`, so a dependency change needs `cargo update --manifest-path harness/rust/Cargo.toml -p <crate>` too, or CI's `--locked` fetch fails.

## Architecture

cliproxy-rs is a drop-in Rust replacement for the Go CLIProxyAPI server: one HTTP endpoint that speaks the Anthropic, OpenAI (Chat, Completions, Responses, WebSocket) and Gemini client APIs and forwards to many upstream providers using a pool of credentials. Go's structure is the map; the Rust crates split it by dependency direction.

### Crate dependency order

```
cpa-core      config.yaml, credential files, ExecRequest/ExecStream envelope, Format, GoStr, static model registry. No networking.
              Its `yaml` module is the workspace's only YAML parser (granit-parser plus a port of yaml.v3's scalar resolution; docs/YAML-PARSER.md).
cpa-common    Go-exact pure helpers: json (gjson/sjson/encoding-json port), thinking, signature, payload rules, session identity, gemini_schema.
cpa-translate (client format, upstream format) pairs: request, non_stream, stream, token_count. Registered with the `registered!` macro in lib.rs.
cpa-exec      one executor per provider (claude, codex, gemini, antigravity, devin, aistudio, ...). Owns wire format, auth headers, TLS/HTTP profile.
cpa-server    axum routes -> dispatch -> runtime (leases, credential store, scheduler) -> executors. Management API, logging, plugins host glue.
cpa-plugin    native plugin host (C ABI, dlopen, host callbacks). cpa-store: PGSTORE/OBJECTSTORE/GITSTORE mirrors. cpa-home: the Home control plane client.
cliproxy      the binary: flags, dotenv, TUI, LAN discovery, Home mode wiring (crates/cliproxy/src/home.rs).
```

### Life of a request

1. `cpa-server/src/lib.rs` registers the routes. Inference routes sit behind `access::require_client_key`; everything else (management, plugin routes, gin's NoRoute 404) is the fallback router passed to `app()`.
2. A route handler (`claude.rs`, `openai.rs`, `gemini.rs`, `realtime.rs`, ...) only parses the client body into a `dispatch::Call` (entry format, expected response format, model, raw `Bytes` body, stream flag) and renders the result.
3. `dispatch.rs` is the provider-neutral attempt loop (Go `BaseAPIHandler.Execute*` plus `Manager.Execute*`): model to providers via `registry.rs`, credential selection via `scheduler.rs` (round-robin, fill-first, weighted, and the Rust-only `soonest-reset`), alias pools, retry rounds, stream bootstrap.
4. `runtime.rs` hands out a `Lease` per attempt. A lease reports exactly one `Outcome` (or `Cancelled` on drop), which feeds cooldowns and usage. The runtime also owns the config snapshot, credential store, refresh loop, OAuth callback sink and the plugin runtime.
5. `cpa_exec::Executors` dispatches on `Credential::provider`. Each executor gets the config snapshot current at request start, calls `cpa_translate::translate_request` when client and upstream formats differ, applies provider rewrites (cloak, tool aliases, thinking, signatures), sends, then reverses the rewrites before the response translator runs. Credential preparation is two-step (`readiness` then `prepare` returning a `MetadataPatch`) so the runtime can single-flight it; executors never write credential files.
6. Streaming: `cpa_translate::sse::Framer` frames upstream events, the pair's `stream` state emits client events, and `stream::frame` writes them the way the Go route handler would.

### Conventions that span crates

- JSON on the wire is read and edited with `cpa_common::json` (byte-exact gjson/sjson behaviour), never deserialised and re-serialised. Bodies are `Bytes` end to end. For large bodies walk once and edit by path (`crates/cpa-exec/src/claude/signals.rs` is the model).
- Behaviour comments cite the Go file (`claude_executor_execute.go`, `server_routes.go`) only where the Rust is non-obvious. Go test names are cited in Rust tests and fixture names because `docs/parity-audit/audit.py` matches them to produce the parity status.
- Goldens: `crates/*/tests/fixtures/` and `crates/*/tests/reference/` hold outputs recorded from the Go reference (the `go_*.jsonl.gz` files replay recorded Go calls exactly). Each fixture keeps its recorded Go version; do not re-record to a newer Go to make a test pass. A deliberate difference needs an entry in `docs/DIFFERENCES-FROM-GO.md` and a test that encodes it.
- Config and credentials are hot-reloaded (`watching.rs`, `fs_events.rs`); a request keeps the snapshot it started with. Lock, refresh and reload code must not hold a lock across an await.
- The optional `cpa-server` features `media-relay` (WebRTC) and `pprof` (CPU profiles) are off in release binaries, and CI fails if `pprof` or `backtrace` enter the default dependency tree.
- The dashboard (`ui/`) talks only to the v8 Management API and must degrade honestly when a route is missing (see `ui/README.md`, `PRODUCT.md`).

## Where to look

| Need | Place |
|---|---|
| What is ported, partial or missing | `docs/PARITY.md`, then `docs/parity-audit/status.md` and `manual.tsv` |
| Deliberate divergences and Rust-only additions | `docs/DIFFERENCES-FROM-GO.md` |
| Commits since the previous reference and their port status | `docs/parity-audit/upstream-v8.0.35.md` |
| Config keys | `crates/cpa-core/src/config.rs`, `docs/CONFIGURATION.md` |
| Performance budgets and how they are measured | `bench/budgets.txt`, `docs/BENCHMARKS.md`, `CONTRIBUTING.md` |
| Claude prompt-cache and telemetry rules | `docs/CLAUDE-CACHE-TELEMETRY.md`, `bench/claude-cache-live.sh` |
| Patched third-party code | `vendor/README.md` |

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:cdcea925 -->
## Beads Issue Tracker

This project uses **bd (beads)** for issue tracking. Run `bd prime` to see full workflow context and commands.

### Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work
bd close <id>         # Complete work
```

### Rules

- Use `bd` for ALL task tracking — do NOT use TodoWrite, TaskCreate, or markdown TODO lists
- Run `bd prime` for detailed command reference and session close protocol
- Use `bd remember` for durable project facts — not per-tool memory files, which fragment across accounts; keep per-operator preferences in your harness's own memory

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.

## Agent Context Profiles

The managed Beads block is task-tracking guidance, not permission to override repository, user, or orchestrator instructions.

- **Conservative (default)**: Use `bd` for task tracking. Do not run git commits, git pushes, or Dolt remote sync unless explicitly asked. At handoff, report changed files, validation, and suggested next commands.
- **Minimal**: Keep tool instruction files as pointers to `bd prime`; use the same conservative git policy unless active instructions say otherwise.
- **Team-maintainer**: Only when the repository explicitly opts in, agents may close beads, run quality gates, commit, and push as part of session close. A current "do not commit" or "do not push" instruction still wins.

## Session Completion

This protocol applies when ending a Beads implementation workflow. It is subordinate to explicit user, repository, and orchestrator instructions.

1. **File issues for remaining work** - Create beads for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **Handle git/sync by active profile**:
   ```bash
   # Conservative/minimal/default: report status and proposed commands; wait for approval.
   git status

   # Team-maintainer opt-in only, unless current instructions forbid it:
   git pull --rebase
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->
