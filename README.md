# Condurence

A native Agentic Development Environment built with **Tauri v2** (Rust) and **React + TypeScript**. It combines:

- **Workspaces:** parallel, isolated `git worktree` workspaces with live diffs and PTY terminals (xterm.js).
- **Agent Orchestration:** a railway-style canvas (React Flow) where agent nodes are wired into routing rules (`delegates_to`, `verifies`, `escalates_to`) and executed through a local MCP router.

The topology is committed to each repository as `.orchestrator/graph.json`. Runtime telemetry lives in `~/.condurence/state.db`.

## Quick start

1. **Install the prerequisites** listed under [Requirements](#requirements). If you don't have Rust yet:

   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   source "$HOME/.cargo/env"
   ```

2. **Install the agent CLIs you plan to use**, then sign in to each once from a terminal. Any install method works, as long as the binary is on your `PATH`:

   ```bash
   npm install -g @anthropic-ai/claude-code   # claude
   npm install -g @openai/codex               # codex
   npm install -g @google/gemini-cli          # gemini
   npm install -g opencode-ai                 # opencode
   ```

3. **Clone and run:**

   ```bash
   git clone https://github.com/dJangg0/Condurrence.git
   cd Condurrence
   npm install
   npm run tauri dev
   ```

   The first run compiles the Rust dependencies, which can take several minutes. Later runs start in seconds, and a Condurence window opens.

4. **Try it** on a git repository that has at least one commit, since a workspace branches from a commit:
   - **Workspaces:** open the repository, create a workspace from a branch name, then launch a Shell or an agent CLI inside it. The diff updates as files change.
   - **Agent Orchestration:** a repository with no topology yet opens with a default one (CEO / Architect, Senior Developer, Debugger & QA). Edit it, press **Save** (⌘S) to write `.orchestrator/graph.json`, pick a root, enter a goal, and press **Run topology**. The root runs in an interactive terminal. Delegated agents appear as read-only streams, and you can inspect one by clicking its node. Approval gates appear in the run log panel.

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

To point Condurence at a specific CLI build, set `CONDURENCE_<PROVIDER>_BIN`, for example `CONDURENCE_CLAUDE_BIN=/opt/tools/claude`. The older `OPENCONDUCTOR_<PROVIDER>_BIN` names still work.

## Gates and retries

A delegated agent's `completed` report is a claim. The repository's **gates** decide whether it stands. Put them in `.orchestrator/gates.json`, which you commit with the topology:

```json
{
  "gates": [
    { "id": "typecheck", "command": "npm run typecheck" },
    { "id": "test", "command": "npm test", "timeoutSec": 900 }
  ]
}
```

- Gates run in order in the child's worktree, after the child has exited. They stop at the first failure.
- A failing gate turns a `completed` report into `failed`. The gate's output is what the caller reads, and each gate run appears in the run log.
- With no file there are no gates. A report is then taken as written.
- Gate commands run through `sh -c`, with the same trust as the agent CLIs. Only commit commands you would run yourself.

Retries are counted per **task**, not per edge for the whole run:

- An agent can pass a `task_id` (letters, digits, `-` and `_`, up to 64 characters). Retries with the same id share one `maxRetries` budget. New work gets a new id.
- A call without an id counts against the caller's stream on that edge, as before.
- The same failure signature twice in a row **stalls** the task, and further attempts are refused until someone changes the plan. The signature is the failing gate and its last output line with digits masked, or the agent's own `failure_signature`.
- A named task whose gates passed is **accepted**, and retrying it under the same id is refused.

## Roles, plans and worktrees

- **Access.** Each agent has `access`: `read-write` (default) or `read-only`. A read-only agent cannot edit files. Claude runs with its edit and shell tools denied, and Codex in its read-only sandbox. Gemini and OpenCode have no read-only mode that is verified here, so for them the runtime's check is the only enforcement. After every read-only run the runtime compares the worktree with its state before the run. A change fails the attempt, whatever the agent reported. The default "Debugger & QA" is read-only: it reviews and reports, and the developer makes the fix.
- **Plans.** The orchestrator can call `plan_tasks` to set the tasks, in order, with what each depends on. Once a run has a plan, every delegation must name a planned task with a `task_id`, and a task waits until the tasks it depends on are accepted. A started task keeps its state through a replan, and a task that has started cannot be dropped.
- **Worktrees.** An isolated agent works in its own branch. Its accepted work is committed and merged into the run's integration branch (`oc/<run>/integration`), one merge at a time, and the next isolated agent starts from that branch's tip. A conflicting merge fails the attempt and names the files, so a retry under the same `task_id` starts from the current tip. A shared agent writes directly in its caller's worktree. A shared worktree takes one writer at a time: a second writer is refused while the first runs, and readers cannot share it with a writer.
- **The run log.** The Tasks tab on the orchestration canvas shows the plan, each attempt with its budget, its state, why a call was refused, the signature of a repeated failure, and the merge result.

## Evals

The runtime is checked by a fixed suite of scripted scenarios that run through the real router with a test double for the CLI:

```bash
cd src-tauri && cargo test --test orchestration_e2e eval_suite -- --nocapture
```

It prints a table of calls, refusals, completions, failures, accepted and stalled tasks, gate failures and merges for each scenario, and writes `condurence-evals/report.json` to the temp directory. Each scenario is also checked against its expected outcome, so a change that alters the runtime's behaviour fails the suite.

The live version uses the real provider CLIs on a repository with a seeded bug. It spends your provider subscription, so it does not run by default:

```bash
cd src-tauri && cargo test --test orchestration_e2e live_eval -- --ignored --nocapture
```

## Where things live

| What | Where |
| --- | --- |
| Topology | `<repo>/.orchestrator/graph.json` (commit this file) |
| Workspaces | `<repo>/.worktrees/` (hidden from `git status` through `.git/info/exclude`) |
| Repo-local skills | `<repo>/.orchestrator/skills/<name>.md`, falling back to `~/.openconductor/skills/<name>.md` |
| Run telemetry | `~/.openconductor/state.db` |
| Run scratch files | `$TMPDIR/openconductor/<run-id>/`, removed when the run stops |

## Troubleshooting

- **"'claude' was not found on your PATH":** Condurence imports your login shell's `PATH` at startup. Check that the CLI runs in a new terminal, then restart the app. To pin a binary instead, set `OPENCONDUCTOR_<PROVIDER>_BIN`, for example `OPENCONDUCTOR_CLAUDE_BIN=/path/to/claude npm run tauri dev`. The providers are `CLAUDE`, `CODEX`, `GEMINI` and `OPENCODE`. The GitHub CLI pins the same way, with `OPENCONDUCTOR_GH_BIN`.
- **Rust build fails on macOS:** run `xcode-select --install`, then open a new terminal.
- **Port 1420 is in use:** another dev server is running. Stop it, because Vite is set to `strictPort`.
- **Workspace creation fails in a new repository:** make at least one commit first.

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
- **Windows:** type-checked for `x86_64-pc-windows-msvc` but not run. Process-tree teardown uses `taskkill /T` rather than Job Objects, and `.cmd` shims are launched through `cmd.exe`.
- **Escalation:** the root orchestrator is interactive, so `escalates_to` into the root starts a fresh headless instance of the root's provider with the escalation context.
- **Merging:** squash-merged branches are not ancestors of their target, so `git branch -d` keeps them. Removing a workspace therefore never discards unmerged work.
