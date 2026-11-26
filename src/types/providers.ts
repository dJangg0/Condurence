/** Mirrors `src-tauri/src/auth/mod.rs`. Sign-in state read from each agent CLI's own status command. */
export type ProviderAuth = {
  /** `claude`, `codex` or `opencode`. */
  provider: string;
  /** False when the CLI cannot be found on PATH. */
  installed: boolean;
  loggedIn: boolean;
  /** Account or plan shown beside the status, such as `ChatGPT`. */
  detail: string | null;
  /** Detail rows when signed in: provider, login method, organization, email. */
  fields: Array<{ label: string; value: string }>;
  /** The resolved CLI executable, when installed. */
  path: string | null;
};
