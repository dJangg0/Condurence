# Software Requirements Specification (SRS): Condurence Multi-Agent Orchestration Subsystem

| Attribute | Value |
| :--- | :--- |
| **Document ID** | `OC-SRS-MAO-001` |
| **Version** | `1.0.0` |
| **Status** | Baseline Specification |
| **Standard Alignment** | ISO/IEC/IEEE 29148:2018 |
| **Target Platforms** | macOS (Apple Silicon & Intel) and Windows 10/11 (x64/ARM64) |
| **Core Stack** | Tauri v2 (Rust), React 19 + TypeScript, `@xyflow/react`, `portable-pty`, SQLite (WAL) |

---

## 1. Introduction

### 1.1 Purpose
This Software Requirements Specification (SRS) defines the functional, interface, data, and non-functional requirements for the **Multi-Agent Orchestration Subsystem** of **Condurence**, structured in accordance with ISO/IEC/IEEE 29148:2018. It serves as the authoritative engineering contract for frontend developers, Rust backend engineers, and autonomous coding agents implementing the orchestration canvas, graph compiler, local Model Context Protocol (MCP) router, and multi-agent execution runtime.

### 1.2 Scope
This specification covers the **Agent Orchestration** module accessible from Condurence's primary sidebar navigation, along with its underlying Rust runtime services:
* **Visual Topology Canvas:** A Railway-inspired infinite drag-and-drop graph field where users instantiate AI agent service nodes, configure personas, skills, context scopes, and CLI providers, and connect nodes via directed relationship edges (analogous to Entity-Relationship Diagrams and n8n workflows).
* **Graph Serialization & Persistence:** Version-controllable repository storage (`.orchestrator/graph.json`) and local execution telemetry (`~/.openconductor/state.db`).
* **Dynamic MCP Compilation & Routing:** An embedded local MCP server and `stdio` proxy that translates canvas edges into callable inter-agent tools for locally authenticated terminal coding agents (`claude`, `codex`, `opencode`, `gemini`).
* **Multi-Agent Execution Engine:** Lifecycle management, Git worktree isolation, retry/loop guards, and human-in-the-loop approval gates.

*Out of Scope:* Single-agent interactive workspace management, side-by-side Git diff rendering, and manual terminal pane layouts (covered under the separate `Workspace ADE` subsystem, though both subsystems share the underlying `pty` and `git` Rust crates).

### 1.3 Definitions, Acronyms, and Abbreviations

| Term / Acronym | Definition |
| :--- | :--- |
| **ADE** | Agentic Development Environment; a local-first IDE/workspace shell designed around autonomous coding agents and isolated Git worktrees. |
| **BYO CLI** | Bring-Your-Own Command-Line Interface; leveraging a user's existing locally authenticated CLI subscriptions rather than wrapping paid third-party API keys. |
| **Canvas** | The interactive 2D node-and-edge graph viewport built on `@xyflow/react`. |
| **Edge Contract** | A directed connection between two Agent Nodes defining delegation, verification, or escalation semantics and execution guards. |
| **MCP** | Model Context Protocol; an open JSON-RPC 2.0 standard enabling LLM agents to discover and invoke external tools and resources. |
| **PTY** | Pseudo-Terminal (`Unix PTY` on macOS, `ConPTY` on Windows) used to host interactive CLI agent processes. |
| **Worktree** | An isolated working directory and Git branch provisioned via `git worktree add` to prevent file collisions across parallel agents. |

---

## 2. Overall Description

### 2.1 System Context & Architecture Overview
The Multi-Agent Orchestration Subsystem operates entirely locally on the user's workstation inside the Condurence Tauri v2 desktop binary. It bridges three layers:
1. **Presentation Layer (WebView — WKWebView / WebView2):** Renders the Railway-style node graph, configuration inspector drawers, live status telemetry, and approval modals.
2. **Orchestration & Routing Layer (Rust `CoreRuntime`):** Compiles graph topologies, enforces loop limits and approval gates, manages ephemeral MCP configurations, and routes JSON-RPC 2.0 tool calls between agents.
3. **Execution & Isolation Layer (OS Subprocesses & Git):** Spawns third-party agent CLIs inside managed PTYs bound to isolated or shared Git worktrees.

### 2.2 Product Functions Summary
* Instantiate, position, duplicate, and delete Agent Nodes on a zoomable, pannable 2D field.
* Bind each node to a distinct CLI provider (`claude`, `codex`, `opencode`, `gemini`), model identifier, role mandate, reusable Markdown skills, and scoped repository file paths.
* Draw directed edges between nodes (e.g., `CEO` → `Senior Developer` ↔ `Debugger`) and configure edge rules (`delegates_to`, `verifies`, `escalates_to`, maximum retry limits, and manual approval gates).
* Persist and reload agent topologies directly within the target Git repository (`.orchestrator/graph.json`).
* Execute a topology against a user objective, dynamically exposing downstream connected agents as MCP tools to upstream agents while streaming real-time node statuses and terminal logs.

### 2.3 User Characteristics
* **Primary Users:** Software engineers, tech leads, and AI workflow builders operating on macOS or Windows who hold active subscriptions to one or more terminal AI coding tools (Claude Code, OpenAI Codex CLI, Gemini CLI, OpenCode) and want deterministic, visual multi-agent collaboration without cloud lock-in.

### 2.4 Operating Environment & Constraints
* **Supported Operating Systems:** macOS 13+ (Ventura or newer) and Windows 10 (build 1903+ with ConPTY support) / Windows 11.
* **Local-First Constraint:** The subsystem shall not require an external cloud backend, account registration, or remote relay server to store graphs or route inter-agent messages.
* **Zero Repository Pollution Constraint:** Temporary MCP configuration files, compiled system prompts, and IPC tokens generated during a run must reside in the OS temporary directory (`std::env::temp_dir()`) and must never appear as untracked files in `git status`.

### 2.5 Assumptions and Dependencies
* **System Git:** `git` (version 2.25 or newer) is installed and accessible on the host machine.
* **Installed Agent CLIs:** At least one supported CLI binary (`claude`, `codex`, `opencode`, or `gemini`) is installed on the host system and pre-authenticated by the user.
* **Protocol Compatibility:** The local MCP bridge implements JSON-RPC 2.0 compatible with the MCP `2026-07-28` stateless specification (supporting `Mcp-Method` / `Mcp-Name` headers and Multi Round-Trip Requests) as well as backward-compatible `stdio` transport for current CLI clients.

---

## 3. External Interface Requirements

### 3.1 User Interfaces
* **UI-01 Sidebar Mode Switcher:** The primary left navigation rail shall provide a persistent **Agent Orchestration** tab alongside **Workspaces**, preserving canvas state and active run telemetry when switching views.
* **UI-02 Railway-Style Service Cards:** Each node on the canvas shall render as a structured card displaying:
  * Agent Name and live execution status indicator (`idle`, `running`, `waiting_approval`, `error`).
  * Assigned Role label and color-coded CLI Provider/Model badge.
  * Attached Skill pills and truncated Context Path scope.
  * Top target handle (incoming edges) and bottom source handle (outgoing edges).
* **UI-03 Contextual Inspector Drawer:** Selecting a node or edge shall open a right-hand inspector panel allowing live editing of all node/edge properties without modal interruptions.
* **UI-04 Live Terminal & Log Drawer:** Clicking an active or completed node's terminal icon shall open an embedded `xterm.js` viewer replaying buffered ANSI output from that node's PTY session.

### 3.2 Software & CLI Provider Interfaces
The Rust backend shall invoke installed CLI binaries using provider-specific adapters:

| Provider Key | CLI Binary | Headless / Delegated Execution Flag | Ephemeral MCP Injection Mechanism |
| :--- | :--- | :--- | :--- |
| `claude` | `claude` | `claude -p "{prompt}" --output-format stream-json` | `--mcp-config {tmp}/claude-mcp.json` |
| `codex` | `codex` | `codex exec "{prompt}"` | `-c mcp_servers.openconductor...` CLI override flags |
| `opencode` | `opencode` | `opencode run "{prompt}"` | `OPENCODE_CONFIG={tmp}/opencode.json` env var |
| `gemini` | `gemini` | `gemini -p "{prompt}"` | Isolated `.gemini/settings.json` MCP server entry |

### 3.3 Inter-Agent Communication Interface (Local MCP Bridge)
* **Loopback HTTP Endpoint:** On startup, the Rust runtime shall bind an `axum` HTTP server strictly to `127.0.0.1:0` (OS-assigned ephemeral port), authenticating requests via a per-run cryptographic bearer token.
* **Self-Binary `stdio` Shim:** Invoking the Condurence binary with `--mcp-bridge --node-id {id} --run-id {run} --port {port} --token {token}` shall run a lightweight JSON-RPC 2.0 `stdio` bridge that forwards `tools/list` and `tools/call` messages from the CLI subprocess to the loopback HTTP endpoint.

### 3.4 Storage & Filesystem Interfaces
* **Graph Definition File:** `{repo_root}/.orchestrator/graph.json` (UTF-8, pretty-printed JSON).
* **Skill Definitions Directory:** `{repo_root}/.orchestrator/skills/{skill_name}.md` with global fallback `~/.openconductor/skills/{skill_name}.md`.
* **Runtime State Database:** `~/.openconductor/state.db` (SQLite in `WAL` mode storing run history, inter-agent call traces, and audit logs).

---

## 4. Specific Functional Requirements

### 4.1 Canvas & Topology Management

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-CANVAS-01** | Must | **Infinite Interactive Field:** The system shall render an infinite 2D canvas supporting pan, zoom (25% to 200%), dot-grid background snapping, minimap overview, and fit-to-view controls.<br>*Verification:* Demonstration of smooth zooming, panning, and node dragging with 20+ nodes on macOS and Windows. |
| **FR-CANVAS-02** | Must | **Node Creation & Removal:** Users shall be able to add new Agent Nodes via a toolbar action or preset templates (`CEO`, `Senior Dev`, `Debugger`, `Reviewer`) and remove nodes (automatically pruning connected edges).<br>*Verification:* Functional UI test confirming node addition, deletion, and orphan-edge cleanup. |
| **FR-CANVAS-03** | Must | **Drag-and-Drop Edge Wiring:** Users shall be able to drag a connection from any node's source handle to another node's target handle to establish a directed communication channel, rejecting self-loops (`source == target`).<br>*Verification:* Unit and UI test verifying valid edge creation and self-loop rejection. |
| **FR-CANVAS-04** | Must | **Repository Graph Persistence:** The system shall automatically load `{repo}/.orchestrator/graph.json` when a repository is opened and persist changes (node positions, configurations, edges) back to `.orchestrator/graph.json`.<br>*Verification:* Integration test saving a modified graph, restarting the app, and verifying exact state restoration. |
| **FR-CANVAS-05** | Should | **Starter Topology Scaffolding:** When opening a repository without an existing `.orchestrator/graph.json`, the system shall offer a one-click starter template pre-wired with `CEO / Architect` → `Senior Developer` ↔ `Debugger & QA`.<br>*Verification:* Demonstration on a fresh Git repository. |

### 4.2 Agent Node Configuration & Provider Binding

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-NODE-01** | Must | **Persona & Role Mandate:** Each node shall store a human-readable `name`, `role` title, and multi-line `systemPrompt` defining its behavioral mandate and decision boundaries.<br>*Verification:* Inspection of compiled instruction markdown generated prior to node launch. |
| **FR-NODE-02** | Must | **Per-Node Provider & Model Selection:** Each node shall independently specify its `provider` (`claude`, `codex`, `opencode`, `gemini`) and `model` string, enabling heterogeneous multi-vendor topologies within a single run.<br>*Verification:* Integration test launching a graph combining at least two distinct CLI providers. |
| **FR-NODE-03** | Must | **Skill Attachment:** Users shall be able to attach zero or more named `skills` to a node. At runtime, the compiler shall resolve matching `.md` skill files and inject their instructions into the agent's context.<br>*Verification:* Test confirming skill file content appears in the node's compiled system instructions. |
| **FR-NODE-04** | Must | **Context Path Scoping:** Each node shall accept a list of relative repository paths (`contextPaths`, e.g., `src-tauri/`, `AGENTS.md`) injected into its system prompt as its primary read/write focus boundary.<br>*Verification:* Inspection of generated prompt payload verifying explicit path constraints. |
| **FR-NODE-05** | Must | **Worktree Isolation Mode:** Each node shall configure `worktreeMode` as either `shared` (operates in the calling upstream agent's worktree) or `isolated` (provisions a dedicated branch/worktree under `.worktrees/`).<br>*Verification:* Filesystem check verifying directory isolation when `isolated` is selected. |

### 4.3 Directed Edge Semantics & Graph Rules

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-EDGE-01** | Must | **Edge Relationship Types:** Each directed edge shall specify a `relationType` from `{ delegates_to, verifies, escalates_to }`, governing the generated MCP tool name, description, and input schema.<br>*Verification:* Unit test on `mcp/compiler.rs` asserting distinct tool schemas per `relationType`. |
| **FR-EDGE-02** | Must | **Retry & Hop Guard Configuration:** Each edge shall define a `maxRetries` integer (default `3`, minimum `1`, maximum `10`) limiting how many times that edge may be traversed within a single orchestration run.<br>*Verification:* Automated test simulating a failing `Senior Dev` ↔ `Debugger` loop and verifying termination after `maxRetries`. |
| **FR-EDGE-03** | Must | **Human-in-the-Loop Gate Flag:** Each edge shall include a boolean `requiresApproval` flag. When `true`, traversing the edge shall suspend execution until the user explicitly approves or rejects the handoff in the UI.<br>*Verification:* Integration test verifying execution blocks on a `tokio::sync::oneshot` channel until UI approval is triggered. |
| **FR-EDGE-04** | Should | **Fan-Out Parallelism:** When an upstream node has multiple outgoing `delegates_to` edges targeting `isolated` worker nodes and invokes them concurrently, the router shall execute those downstream agents in parallel.<br>*Verification:* Timing and process test confirming concurrent PTY execution across two isolated worktrees. |

### 4.4 Dynamic MCP Compilation & Inter-Agent Bridge

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-MCP-01** | Must | **Edge-to-Tool Projection:** For every outgoing edge `(A → B)`, the MCP compiler shall dynamically register an MCP tool on Node `A`'s scoped endpoint (`delegate_to_{b_id}`, `verify_with_{b_id}`, or `escalate_to_{b_id}`) accepting `{ task, acceptance_criteria, target_files }`.<br>*Verification:* Calling `tools/list` on the `--mcp-bridge` for Node `A` returns only tools matching `A`'s outgoing edges. |
| **FR-MCP-02** | Must | **Explicit Completion Tool:** Every delegated downstream node shall receive an injected `report_task_result(status, summary, artifacts)` MCP tool so agents can deterministically signal completion even before CLI process teardown.<br>*Verification:* Test verifying `report_task_result` captures structured output and resolves the upstream call. |
| **FR-MCP-03** | Must | **Synchronous Handoff Blocking:** When Node `A` invokes an edge tool targeting Node `B`, the MCP server shall hold Node `A`'s `tools/call` HTTP/stdio request open until Node `B` completes its execution turn, returning Node `B`'s summary and `git diff --stat` as the tool result.<br>*Verification:* End-to-end test verifying `CEO` receives `Senior Dev`'s diff summary inside its tool response. |
| **FR-MCP-04** | Must | **Scoped Authentication:** Every `--mcp-bridge` invocation shall authenticate against the local `axum` server using a unique per-run token; unauthorized requests or cross-run node IDs shall be rejected with HTTP `401`/`403`.<br>*Verification:* Security unit test sending invalid tokens to the loopback port. |

### 4.5 Execution Runtime, Loop Guards & Human-in-the-Loop Gates

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-EXEC-01** | Must | **Topology Run Initialization:** Triggering **Run Topology** with a root node (defaulting to the node with zero incoming `delegates_to` edges, e.g., `CEO`) and a user prompt shall create a new `run_id` in SQLite, provision the root worktree, and launch the root agent.<br>*Verification:* Demonstration starting a run and verifying database record and worktree creation. |
| **FR-EXEC-02** | Must | **Real-Time Canvas Telemetry:** The runtime shall emit `node-status://{node_id}` events (`idle`, `running`, `waiting_approval`, `error`) and edge traversal animations to the frontend within 100 ms of state transitions.<br>*Verification:* UI inspection during live multi-agent execution. |
| **FR-EXEC-03** | Must | **Cycle & Retry Enforcement:** The router shall track `edge_attempts` per `edge_id` in `ExecutionContext`. When an invocation exceeds `edge.maxRetries`, the router shall not spawn the target agent and shall instead return a structured error message instructing the caller to escalate or summarize the blocker.<br>*Verification:* Unit test verifying graceful refusal on attempt `maxRetries + 1`. |
| **FR-EXEC-04** | Must | **Global Run Cancellation:** Clicking **Stop Run** in the UI shall trigger a `tokio_util::sync::CancellationToken`, immediately terminating all active PTY process trees associated with `run_id` and cleaning up temporary MCP config directories.<br>*Verification:* Process table inspection on macOS/Windows confirming zero surviving child processes after cancellation. |
| **FR-EXEC-05** | Must | **Execution Audit Log Persistence:** All edge invocations, input payloads, timestamps, retry counts, and returned summaries shall be persisted in `~/.openconductor/state.db` for post-run inspection.<br>*Verification:* SQLite query check after completing a multi-hop run. |

### 4.6 Worktree Isolation & Conflict Prevention

| ID | Priority | Requirement & Verification Criteria |
| :--- | :--- | :--- |
| **FR-GIT-01** | Must | **Repository-Level Git Mutation Lock:** All mutating Git operations (`worktree add`, `worktree remove`, `branch`) shall acquire a per-repository `tokio::sync::Semaphore` (1 permit) keyed by canonical repository path to prevent `.git/index.lock` failures during parallel fan-out.<br>*Verification:* Stress test spawning 5 parallel isolated nodes simultaneously without Git lock errors. |
| **FR-GIT-02** | Must | **Windows Path & Line-Ending Safety:** All worktree creation commands shall pass `-c core.longpaths=true` and `-c core.autocrlf=input` to `git`.<br>*Verification:* Unit test inspecting generated `git` command arguments. |
| **FR-GIT-03** | Should | **Worktree Merge-Back Action:** Upon completion of an orchestration run, the user shall be able to merge or squash-merge the resulting worktree branch back into the repository's base branch from the UI.<br>*Verification:* End-to-end test merging a multi-agent feature branch cleanly. |

---

## 5. Non-Functional Requirements

| ID | Category | Requirement & Target Metric |
| :--- | :--- | :--- |
| **NFR-PERF-01** | Performance | **Canvas Rendering:** The React Flow canvas shall maintain at least 60 FPS during drag, pan, and zoom operations with up to 30 agent nodes and 50 edges on standard integrated graphics. |
| **NFR-PERF-02** | Performance | **MCP Bridge Latency:** Local IPC routing overhead between a CLI `--mcp-bridge` call and the Rust `axum` router shall be less than 15 ms (excluding target LLM inference time). |
| **NFR-PERF-03** | Performance | **PTY Buffer Memory Cap:** Each active PTY session shall cap its in-memory ANSI replay ring buffer at 256 KB to prevent memory bloat during verbose build logs. |
| **NFR-REL-01** | Reliability | **Zero Orphan Processes:** Terminating an agent node, cancelling a run, or exiting Condurence shall reap 100% of descendant processes via Win32 Job Objects (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) on Windows and process-group signals (`killpg`) on macOS. |
| **NFR-REL-02** | Reliability | **Login-Shell `PATH` Resolution:** On macOS GUI startup, the backend shall resolve the user's full login-shell `PATH` within 2 seconds so Homebrew, NVM, and Cargo binaries are reliably located. |
| **NFR-SEC-01** | Security | **Loopback-Only Binding:** The local MCP HTTP server shall bind exclusively to `127.0.0.1` and never expose a network interface on `0.0.0.0` or LAN adapters. |
| **NFR-SEC-02** | Security | **Credential Isolation:** Condurence shall never read, extract, or store raw OAuth tokens or API keys belonging to `claude`, `codex`, `opencode`, or `gemini`; authentication remains entirely managed by each vendor's CLI. |
| **NFR-COMP-01** | Compatibility | **Cross-Platform Shim Handling:** On Windows, the binary resolver shall transparently resolve `.cmd` and `.exe` CLI wrappers in `%APPDATA%\npm` without command-line injection or newline truncation. |

---

## 6. Data Schemas & State Machine Reference

### 6.1 `.orchestrator/graph.json` Schema

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "version": 1,
  "nodes": [
    {
      "id": "ceo",
      "type": "agentNode",
      "position": { "x": 260, "y": 40 },
      "data": {
        "name": "CEO / Architect",
        "role": "Primary Orchestrator",
        "provider": "claude",
        "model": "opus",
        "skills": ["spec-planner", "task-router"],
        "contextPaths": ["AGENTS.md", "docs/"],
        "systemPrompt": "Decompose requirements into scoped tasks and delegate via MCP tools.",
        "worktreeMode": "shared",
        "status": "idle"
      }
    },
    {
      "id": "senior-dev",
      "type": "agentNode",
      "position": { "x": 100, "y": 260 },
      "data": {
        "name": "Senior Developer",
        "role": "Implementation Engineer",
        "provider": "codex",
        "model": "gpt-5-codex",
        "skills": ["fullstack-ts", "rust-tauri"],
        "contextPaths": ["src/", "src-tauri/"],
        "systemPrompt": "Implement delegated tasks cleanly and verify changes with Debugger.",
        "worktreeMode": "shared",
        "status": "idle"
      }
    },
    {
      "id": "debugger",
      "type": "agentNode",
      "position": { "x": 420, "y": 260 },
      "data": {
        "name": "Debugger & QA",
        "role": "Verification & Fix",
        "provider": "gemini",
        "model": "gemini-2.5-pro",
        "skills": ["test-runner", "stacktrace-analyzer"],
        "contextPaths": ["tests/", "src/"],
        "systemPrompt": "Run builds/tests, isolate failures, patch regressions, and report results.",
        "worktreeMode": "shared",
        "status": "idle"
      }
    }
  ],
  "edges": [
    {
      "id": "e-ceo-dev",
      "source": "ceo",
      "target": "senior-dev",
      "data": {
        "relationType": "delegates_to",
        "maxRetries": 3,
        "requiresApproval": false
      }
    },
    {
      "id": "e-dev-debug",
      "source": "senior-dev",
      "target": "debugger",
      "data": {
        "relationType": "verifies",
        "maxRetries": 3,
        "requiresApproval": false
      }
    }
  ]
}
```

### 6.2 Agent Node Execution State Machine

```text
  [idle] ──(MCP tool call / Run start)──→ [waiting_approval] (if requiresApproval == true)
    ↑                                              │
    │                                        (user approves)
    │                                              ↓
    ├──(Exit 0 / report_task_result)───────── [running]
    │                                              │
    └──(User resets / retry succeeds)────── [error] ←──(Non-zero exit / timeout / rejected)
```

---

## 7. Requirements Traceability & Verification Matrix

Aligned with ISO/IEC/IEEE 29148:2018 verification methods (`Inspection`, `Demonstration`, `Test`):

| Requirement Group | Primary Subsystem Module | Verification Method | Target Acceptance Artifact |
| :--- | :--- | :--- | :--- |
| `FR-CANVAS-01` to `05` | `src/components/OrchestrationCanvas.tsx` | Demonstration & Test | Playwright/Vitest canvas interaction & `.orchestrator/graph.json` round-trip test |
| `FR-NODE-01` to `05` | `src/components/AgentNode.tsx`, `mcp/compiler.rs` | Inspection & Test | Unit test asserting compiled system prompt markdown and provider flags |
| `FR-EDGE-01` to `04` | `mcp/compiler.rs`, `mcp/router.rs` | Test | Rust unit tests for schema projection, `maxRetries` cutoff, and parallel fan-out |
| `FR-MCP-01` to `04` | `mcp/server.rs`, `src-tauri/src/main.rs` | Test | JSON-RPC 2.0 integration test exercising `--mcp-bridge` against mock CLI |
| `FR-EXEC-01` to `05` | `runtime/mod.rs`, `runtime/state_db.rs` | Demonstration & Test | End-to-end 3-node run (`CEO` → `Senior Dev` ↔ `Debugger`) verified in SQLite |
| `FR-GIT-01` to `03` | `git/worktree.rs` | Test | Concurrent worktree creation stress test on macOS and Windows |
| `NFR-PERF-*` / `NFR-REL-*` | `pty/process_group.rs`, `env/resolver.rs` | Test & Inspection | OS process-tree teardown test (Job Object / `killpg`) and frame-rate profiling |