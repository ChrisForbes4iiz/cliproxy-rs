# cliproxy-rs

If you are an agent installing cliproxy-rs for a user rather than developing it, stop here and follow [docs/AI-SETUP.md](docs/AI-SETUP.md).

A Rust rewrite of CLIProxyAPI (https://github.com/router-for-me/CLIProxyAPI) targeting full parity and drop-in compatibility: same `config.yaml`, same auth JSON files in `auth-dir`, same HTTP routes and response shapes, same v8 Management API.

## Reference

- The reference is frozen at CLIProxyAPI **v8.0.35**, commit `ae3ee3be303fb07fb8173279923413005bf32177`: clone https://github.com/router-for-me/CLIProxyAPI, check out that commit and treat the clone as read-only. Cite Go file paths in comments only where behaviour is non-obvious. Existing goldens keep their recorded versions; they do not prove later Go changes. `docs/parity-audit/checklist.md` tracks the target contracts; `docs/parity-audit/upstream-v8.0.35.md` lists the commits since the previous reference, `6fecc6e`.
- When Go behaviour and documentation disagree, the Go code wins. Port behaviour, not structure: idiomatic Rust over transliterated Go.
- A deliberate difference from Go is listed in `docs/DIFFERENCES-FROM-GO.md`, and tests that compare against Go goldens encode it explicitly.

## Layout

- `crates/cpa-core`: config and credential file formats. No networking.
- `crates/cpa-common`: provider-neutral, Go-exact helpers shared by translators and executors.
- `crates/cpa-translate`: format translation between the Anthropic, OpenAI and Gemini APIs.
- `crates/cpa-exec`: one module per upstream provider (wire format, auth headers, HTTP client profile).
- `crates/cpa-server`: axum routes, client auth, credential selection, management API.
- `crates/cpa-plugin`, `crates/cpa-store`, `crates/cpa-home`: the plugin host, the remote config and auth stores, and Home mode.
- `crates/cliproxy`: the binary. Go-style single-dash flags are accepted.
- `ui/` is the dashboard, `harness/` sends the same requests to Go and Rust and compares them.

## Rules

- The full test suite is slow and memory-heavy (the BoringSSL build and the release link). While iterating, run the affected crate or a filter, for example `cargo test -p cpa-exec kimi`; CI runs fmt, `clippy -D warnings` and the whole suite. On a shared or small machine, add `CARGO_BUILD_JOBS=2 nice -n 19`.
- The Go and PostgreSQL comparison tests in `cargo test` run fully only in CI, inside a network-denied namespace. For a local Go/Rust differential run on Linux, use `./harness/run` (see `harness/README.md`).
- Port Go's `strings.ToLower`, `strings.ToUpper` and `strings.EqualFold` with `cpa_core::gostr::GoStr` (`go_lower`, `go_upper`, `go_eq_fold`). Rust's full case mappings differ on non-ASCII input, so `clippy.toml` disallows `str::to_lowercase` and `str::to_uppercase`; keep `eq_ignore_ascii_case` only where the input is ASCII by definition (header names, protocol tokens) or the comparison is explicitly ASCII. An ASCII literal alone is not enough: Go's `EqualFold` also matches `ſ` to `s` and the Kelvin sign `K` to `k`.
- Request bodies are forwarded byte for byte unless a ported rule rewrites them. Never re-serialize JSON just to pass it through.
- Upstream credentials and client keys must never be logged or forwarded to the wrong side.
- Upstream request and response capture goes through the shared sink in `crates/cpa-server/src/request_logging.rs`. It redacts `Cookie` and `Set-Cookie` values, credentials in URLs and short keys, and spools large sections to disk. Don't format or log captured bytes anywhere else.
- Keep request preparation linear in the body size. Coding agents send bodies from 100 KB to several MB, so walk a JSON body once and edit by path (see `crates/cpa-exec/src/claude/signals.rs`) instead of searching it again from the start for each element.
- Performance and memory claims come from `docs/BENCHMARKS.md`. `bench/run.sh`, `bench/messages.sh` and `bench/latency.sh` reproduce its scripted workloads; field observations there are labelled as such and are not reproducible on demand.
- Tests use local mock upstreams and never send traffic to real provider accounts. Give every test a private `auth-dir`: without one, credential loading falls back to `~/.cli-proxy-api`, where real logins live. Build test runtimes with `cpa_server::testing::runtime`.
- Mark deliberate simplifications with a `ponytail:` comment naming the ceiling and the upgrade path.
- Prompt-cache changes must keep request semantics and stable prefixes, and show cache-read and cache-write usage for warm, changed-context and idle continuations before merge. Never add inference keepalive pings. `bench/claude-cache-live.sh` runs that check against a live Claude login, by hand.

## Review guidelines

Flag, in order: behaviour that differs from Go v8.0.35 (`ae3ee3be`) without an entry in `docs/DIFFERENCES-FROM-GO.md` or an explicit partial/missing audit item; request or response bytes changed on passthrough; credentials or client keys reaching logs or the wrong side; tests that can touch the network or a real `auth-dir`; work on a request body that grows faster than linearly with its size. Skip style nits that `cargo fmt` and clippy already cover.

## Claude cache telemetry deployment check

Publishing a release or replacing Core's running proxy requires Vayun's explicit
approval. Mock tests do not prove that Core's collector retains the new fields.
After an approved deployment, explicitly enable `observability.usage.claude-cache-telemetry`
alongside usage statistics. Inspect the collector's protected JSONL file without
printing legacy credential fields (hashes remain stable only until proxy restart):

```sh
jq -c 'select(.claude_cache.schema_version == 2) | {execution_id, usage_observed, tokens: {cache_read_tokens: .tokens.cache_read_tokens, cache_creation_5m_tokens: .tokens.cache_creation_5m_tokens, cache_creation_1h_tokens: .tokens.cache_creation_1h_tokens}, claude_cache}' "$USAGE_JSONL"
```

Confirm that normal existing Amp traffic produces ordered start, first-byte, and
end timestamps, final TTLs, stable hashes, and returned TTL buckets. Check both
buffered and streaming requests if both occur naturally. Do not send synthetic
requests to a live login for telemetry-only deployment checks. This rule does not
replace the required live cache-read/cache-write check above for changes to cache
semantics. The usage queue holds at most one hour, so a week-long
study needs a continuous collector and a separate hashed schedule export. See
[Claude cache telemetry](docs/CLAUDE-CACHE-TELEMETRY.md) for replay and limitations.

<!-- BEGIN BEADS INTEGRATION v:1 profile:minimal hash:6ffdf896 -->
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
   bd dolt push
   git push
   git status
   ```
5. **Hand off** - Summarize changes, validation, issue status, and any blocked sync/commit/push step

**Critical rules:**
- Explicit user or orchestrator instructions override this Beads block.
- Do not commit or push without clear authority from the active profile or the current user request.
- If a required sync or push is blocked, stop and report the exact command and error.
<!-- END BEADS INTEGRATION -->

<!-- BEGIN BEADS CODEX SETUP: generated by bd setup codex -->
## Beads Issue Tracker

Use Beads (`bd`) for durable task tracking in repositories that include it. Use the `beads` skill at `.agents/skills/beads/SKILL.md` (project install) or `~/.agents/skills/beads/SKILL.md` (global install) for Beads workflow guidance, then use the `bd` CLI for issue operations.

### Quick Reference

```bash
bd ready                # Find available work
bd show <id>            # View issue details
bd update <id> --claim  # Claim work
bd close <id>           # Complete work
bd prime                # Refresh Beads context
```

### Rules

- Use `bd` for all task tracking; do not create markdown TODO lists.
- Run `bd prime` when Beads context is missing or stale. Codex 0.129.0+ can load Beads context automatically through native hooks; use `/hooks` to inspect or toggle them.
- Keep durable project facts in Beads via `bd remember` rather than in per-tool memory files, which fragment across accounts; per-operator preferences belong in your harness's own memory.

**Architecture in one line:** issues live in a local Dolt DB; sync uses `refs/dolt/data` on your git remote; `.beads/issues.jsonl` is a passive export. See https://github.com/gastownhall/beads/blob/main/docs/core-concepts/sync-concepts.md for details and anti-patterns.
<!-- END BEADS CODEX SETUP -->
