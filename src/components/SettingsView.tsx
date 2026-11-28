import { useState } from "react";
import { PlayIcon, RefreshIcon, TerminalIcon } from "./icons";
import { SignInDialog, startProviderSignIn } from "./SignInDialog";
import type { Connections } from "../lib/connections";
import type { ProviderAuth } from "../types/providers";

type AgentTab = {
  id: string;
  label: string;
  /** The command the Connect button runs, shown on the button. */
  loginCommand: string;
  /** Environment variable that pins a different executable. */
  pathVariable: string;
};

const AGENTS: AgentTab[] = [
  { id: "claude", label: "Claude Code", loginCommand: "claude auth login", pathVariable: "CONDURENCE_CLAUDE_BIN" },
  { id: "codex", label: "Codex", loginCommand: "codex login", pathVariable: "CONDURENCE_CODEX_BIN" },
  { id: "opencode", label: "OpenCode", loginCommand: "opencode auth login", pathVariable: "CONDURENCE_OPENCODE_BIN" },
];

const BUTTON_QUIET =
  "flex items-center gap-2 rounded-md border border-zinc-700 px-3 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50";

/** Settings page. Today it holds the Agents tab: sign-in state and the executable for each agent CLI. */
export function SettingsView({ connections }: { connections: Connections }) {
  const [tab, setTab] = useState(AGENTS[0].id);
  const [signIn, setSignIn] = useState<{ sessionId: string; title: string } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const agent = AGENTS.find((item) => item.id === tab) ?? AGENTS[0];
  const status = connections.providers?.find((item) => item.provider === agent.id) ?? null;

  const runLogin = async () => {
    setError(null);
    try {
      const sessionId = await startProviderSignIn(agent.id);
      setSignIn({ sessionId, title: `Sign in to ${agent.label}` });
    } catch (e) {
      setError(String(e));
    }
  };

  const finishSignIn = () => {
    setSignIn(null);
    connections.refresh();
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl px-8 py-8">
        <h1 className="text-2xl font-semibold tracking-tight text-zinc-100">Agents</h1>

        <div role="tablist" aria-label="Agents" className="mt-6 flex gap-1 border-b border-zinc-800">
          {AGENTS.map((item) => (
            <button
              key={item.id}
              type="button"
              role="tab"
              aria-selected={tab === item.id}
              onClick={() => {
                setTab(item.id);
                setError(null);
              }}
              className={`-mb-px border-b-2 px-3 py-2 text-sm ${
                tab === item.id
                  ? "border-zinc-300 text-zinc-100"
                  : "border-transparent text-zinc-500 hover:text-zinc-300"
              }`}
            >
              {item.label}
            </button>
          ))}
        </div>

        <section className="mt-6 space-y-6">
          <div>
            <h2 className="text-sm font-medium text-zinc-100">Authentication</h2>
            <p className="mt-1 text-xs text-zinc-500">
              {agent.label} signs in through its own CLI. Condurence only reads the result and never stores your tokens.
            </p>
          </div>

          <div className="flex items-center gap-2">
            <StatusBadge status={status} loading={connections.providers === null} />
            <button
              type="button"
              onClick={connections.refresh}
              className="ml-auto flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200"
            >
              <RefreshIcon />
              Refresh
            </button>
          </div>

          {status?.loggedIn && status.fields.length > 0 ? (
            <dl className="overflow-hidden rounded-lg border border-zinc-800">
              {status.fields.map((row, index) => (
                <div
                  key={row.label}
                  className={`flex text-sm ${index > 0 ? "border-t border-zinc-800" : ""}`}
                >
                  <dt className="w-40 shrink-0 bg-zinc-900/60 px-4 py-2.5 text-zinc-500">{row.label}</dt>
                  <dd className="min-w-0 flex-1 truncate px-4 py-2.5 text-zinc-100">{row.value}</dd>
                </div>
              ))}
            </dl>
          ) : null}

          {status?.installed ? (
            <button type="button" onClick={() => void runLogin()} className={BUTTON_QUIET}>
              <PlayIcon />
              Run {agent.loginCommand}
            </button>
          ) : null}

          {error ? (
            <div className="rounded-md border border-red-500/30 bg-red-500/10 px-3 py-2 text-xs text-red-200">{error}</div>
          ) : null}

          <div className="border-t border-zinc-800 pt-6">
            <h2 className="text-sm font-medium text-zinc-100">{agent.label} executable</h2>
            <ExecutableInfo status={status} variable={agent.pathVariable} installHint={agent.id} />
          </div>
        </section>
      </div>

      {signIn ? <SignInDialog title={signIn.title} sessionId={signIn.sessionId} onDone={finishSignIn} /> : null}
    </div>
  );
}

function StatusBadge({ status, loading }: { status: ProviderAuth | null; loading: boolean }) {
  const [dot, text] = loading
    ? ["bg-zinc-500", "Checking…"]
    : !status || !status.installed
      ? ["bg-zinc-500", "CLI not installed"]
      : status.loggedIn
        ? ["bg-emerald-400", "Connected"]
        : ["bg-amber-400", "Not connected"];
  return (
    <span className="flex items-center gap-2 text-sm text-zinc-100">
      <span className={`h-2 w-2 rounded-full ${dot}`} />
      {text}
    </span>
  );
}

function ExecutableInfo({
  status,
  variable,
  installHint,
}: {
  status: ProviderAuth | null;
  variable: string;
  installHint: string;
}) {
  if (!status?.installed || !status.path) {
    return (
      <p className="mt-2 text-xs text-zinc-500">
        <code className="font-mono text-zinc-300">{installHint}</code> was not found on your PATH. Install it, then refresh.
      </p>
    );
  }
  return (
    <>
      <div className="mt-3 flex items-center gap-2 rounded-md border border-zinc-800 bg-zinc-950 px-3 py-2">
        <TerminalIcon className="shrink-0 text-zinc-500" />
        <span className="min-w-0 flex-1 truncate font-mono text-xs text-zinc-200" title={status.path}>
          {status.path}
        </span>
      </div>
      <p className="mt-2 text-xs text-zinc-500">
        Found on your PATH. To use a different build, set <code className="font-mono text-zinc-300">{variable}</code> before
        launching Condurence.
      </p>
    </>
  );
}
