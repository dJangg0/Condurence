import type { AgentProvider } from "../types/orchestration";

export type WorkspaceLauncher = { id: AgentProvider | "shell"; label: string };

/** The terminals a workspace can start. The workspace view offers them. */
export const LAUNCHERS: WorkspaceLauncher[] = [
  { id: "shell", label: "Shell" },
  { id: "claude", label: "Claude" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
  { id: "opencode", label: "OpenCode" },
];
