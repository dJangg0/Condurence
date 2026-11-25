# OpenConductor

A native Agentic Development Environment built with **Tauri v2** (Rust) and **React + TypeScript**. It combines:

- **Workspaces:** parallel, isolated `git worktree` workspaces with live diffs and PTY terminals (xterm.js).
- **Agent Orchestration:** a railway-style canvas (React Flow) where agent nodes are wired into routing rules (`delegates_to`, `verifies`, `escalates_to`) and executed through a local MCP router.

The topology is committed to each repository as `.orchestrator/graph.json`. Runtime telemetry lives in `~/.openconductor/state.db`.

## Requirements

- Node.js 22+ and npm
- Rust stable (`rustup`)
- git
- macOS: Xcode Command Line Tools. Windows is not yet verified (see Status).
- Provider CLIs for the agents you use (`claude`, `codex`, `gemini`, `opencode`) on your `PATH`. OpenConductor wraps the CLIs you are already signed into; it does not call model APIs directly.

## Run

```bash
npm install
npm run tauri dev       # launches the desktop app
npm run tauri build     # production bundle
```

## Test

```bash
npm test                 # frontend: topology conversion and validation
npm run typecheck
cd src-tauri && cargo test
```

`cargo test` includes an end-to-end orchestration test. It runs the real PTY, loopback MCP server, `open-conductor --mcp-bridge` stdio shim, router and state DB, with a Python stand-in for the `claude` CLI. It covers delegation, the isolated child worktree, the `maxRetries` budget, and approve and reject on approval gates.

To point OpenConductor at a specific CLI build, set `OPENCONDUCTOR_<PROVIDER>_BIN`, for example `OPENCONDUCTOR_CLAUDE_BIN=/opt/tools/claude`.

## Architecture

| Area | Location | Responsibility |
| --- | --- | --- |
| Topology schema | `src-tauri/src/graph`, `src/types/orchestration.ts` | Canonical `graph.json` contract, validation, atomic save |
| Git | `src-tauri/src/git` | Worktree lifecycle, per-repo mutation locks, structured diffs, debounced watcher |
| Terminals | `src-tauri/src/pty` | portable-pty sessions, 256 KB replay ring, process-group teardown |
| Orchestration | `src-tauri/src/mcp` | Graph-to-MCP compiler, stdio bridge, loopback server, handoff router |
| Persistence | `src-tauri/src/runtime/state_db.rs` | SQLite (WAL): repos, runs, journaled events, PTY sessions |
| UI | `src/components`, `src/App.tsx` | Workspace view, orchestration canvas, inspector, terminal and diff panes |

Each delegated handoff runs headless in a visible PTY with an injected `report_task_result` tool. The caller's `tools/call` stays pending until the child reports or exits, and then returns the summary, exit code and `git diff --stat`.

## Status and known limits

- **Windows:** the code is written but not compiled or run on Windows here. Process-tree teardown uses `taskkill /T` rather than Job Objects, and `.cmd` shims are launched through `cmd.exe`.
- **Provider adapters:** the Claude and Codex flags were checked against the installed CLIs' `--help`. The Gemini and OpenCode adapters are unverified because those CLIs are not installed here. Live runs against authenticated subscriptions were not performed.
- **Escalation:** the root orchestrator is interactive, so `escalates_to` into the root starts a fresh headless instance of the root's provider with the escalation context.
- **Merging:** squash-merged branches are not ancestors of their target, so `git branch -d` keeps them. Removing a workspace therefore never discards unmerged work.
