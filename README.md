# Condurence

A native Agentic Development Environment built with **Tauri v2** (Rust) and **React + TypeScript**. It combines:

- **Workspaces:** parallel, isolated `git worktree` workspaces with live diffs and PTY terminals (xterm.js).
- **Agent Orchestration:** a railway-style canvas (React Flow) where agent nodes are wired into routing rules (`delegates_to`, `verifies`, `escalates_to`) and executed through a local MCP router.

The topology is committed to each repository as `.orchestrator/graph.json`. Runtime telemetry lives in `~/.openconductor/state.db`.

## Requirements

- Node.js 22+ and npm
- Rust stable (`rustup`)
- git
- macOS: Xcode Command Line Tools. Windows is not yet verified (see Status).
- Provider CLIs for the agents you use (`claude`, `codex`, `gemini`, `opencode`) on your `PATH`. Condurence wraps the CLIs you are already signed into; it does not call model APIs directly.
- Optional: GitHub CLI (`gh`) for the GitHub connection in the header. Sign in from the app, or run `gh auth login` yourself. Condurence reads sign-in state through `gh` and never sees your token.

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

`cargo test` includes an end-to-end orchestration test. It runs the real PTY, loopback MCP server, `Condurence --mcp-bridge` stdio shim, router and state DB, with a Python stand-in for the `claude` CLI. It covers delegation, the isolated child worktree, the `maxRetries` budget, and approve and reject on approval gates.

To point Condurence at a specific CLI build, set `OPENCONDUCTOR_<PROVIDER>_BIN`, for example `OPENCONDUCTOR_CLAUDE_BIN=/opt/tools/claude`.

## Specification

The baseline requirements for the multi-agent orchestration subsystem are in [`docs/SRS-multi-agent-orchestration.md`](docs/SRS-multi-agent-orchestration.md) (`OC-SRS-MAO-001`, ISO/IEC/IEEE 29148 structure). Requirement IDs such as `FR-EDGE-02` are the reference for tests and changes.

## Architecture

| Area | Location | Responsibility |
| --- | --- | --- |
| Topology schema | `src-tauri/src/graph`, `src/types/orchestration.ts` | Canonical `graph.json` contract, validation, atomic save |
| Git | `src-tauri/src/git` | Worktree lifecycle, per-repo mutation locks, structured diffs, debounced watcher |
| GitHub | `src-tauri/src/github` | Account state via `gh`, linking a repository by its `origin` GitHub remote, sign-in through `gh auth login` |
| Terminals | `src-tauri/src/pty` | portable-pty sessions, 256 KB replay ring, process-group teardown |
| Orchestration | `src-tauri/src/mcp` | Graph-to-MCP compiler, stdio bridge, loopback server, handoff router |
| Persistence | `src-tauri/src/runtime/state_db.rs` | SQLite (WAL): repos, runs, journaled events, PTY sessions |
| UI | `src/components`, `src/App.tsx` | Workspace view, orchestration canvas, inspector, terminal and diff panes |

Each delegated handoff runs headless in a visible PTY with an injected `report_task_result` tool. The caller's `tools/call` stays pending until the child reports or exits, and then returns the summary, exit code and `git diff --stat`.

## Status and known limits

- **Provider adapters:** flags for Claude, Codex, Gemini and OpenCode were checked against each CLI's `--help` output and, for the environment variables, against the CLI sources. No live run against an authenticated subscription has been done yet.
- **Permissions in delegated runs:** headless Claude uses `--permission-mode acceptEdits`, so shell commands such as test runs may be denied. Headless OpenCode uses `--auto`, which auto-approves every permission request. Worktree isolation limits the blast radius, but this is a real trust decision.
- **Gemini sign-in:** `GEMINI_CLI_HOME` relocates the whole Gemini home. The ephemeral home therefore symlinks your real `~/.gemini` entries and only writes its own `settings.json`. On Windows symlinks need extra privileges, so delegated Gemini runs there start signed out.
- **Windows:** not yet compiled for Windows. Process-tree teardown uses `taskkill /T` rather than Job Objects, and `.cmd` shims are launched through `cmd.exe`.
- **Escalation:** the root orchestrator is interactive, so `escalates_to` into the root starts a fresh headless instance of the root's provider with the escalation context.
- **Merging:** squash-merged branches are not ancestors of their target, so `git branch -d` keeps them. Removing a workspace therefore never discards unmerged work.
