import { useEffect, useRef, useState } from "react";
import { CheckIcon, ChevronUpDownIcon } from "./icons";
import { SignInDialog, startGitHubSignIn, startProviderSignIn } from "./SignInDialog";
import { TokenUsageSection, USAGE_SUMMARY_DAYS } from "./TokenUsageSection";
import { ipc } from "../lib/ipc";
import type { Connections } from "../lib/connections";
import type { Preferences, ThemeChoice } from "../lib/preferences";
import type { ProviderUsage } from "../types/orchestration";

const THEMES: Array<{ id: ThemeChoice; label: string }> = [
  { id: "system", label: "System" },
  { id: "light", label: "Light" },
  { id: "dark", label: "Dark" },
];

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500";

const SECTION_LABEL = "px-2 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wider text-zinc-500";

/** One account row. `id` is `github` or a provider id (`claude`, `codex`, `opencode`). */
type AccountRow = {
  id: string;
  label: string;
  monogram: string;
  state: "checking" | "missing" | "signedOut" | "signedIn";
  detail: string | null;
};

const PROVIDER_ROWS: Array<{ id: string; label: string; monogram: string }> = [
  { id: "claude", label: "Claude Code", monogram: "C" },
  { id: "codex", label: "ChatGPT (Codex)", monogram: "G" },
  { id: "opencode", label: "OpenCode", monogram: "O" },
];

function initialsOf(name: string): string {
  return name
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0])
    .join("")
    .toUpperCase();
}

/**
 * Bottom-left account button. Opens a popover with the profile name, the connected accounts
 * (GitHub and the agent CLIs, each signed in through its own CLI), and appearance settings.
 */
export function ProfileMenu({
  preferences,
  onChange,
  connections,
}: {
  preferences: Preferences;
  onChange: (patch: Partial<Preferences>) => void;
  connections: Connections;
}) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const [signIn, setSignIn] = useState<{ sessionId: string; title: string } | null>(null);
  const [signInError, setSignInError] = useState<string | null>(null);
  // Token usage is read when the pane opens, not on every render, because it scans session logs.
  const [usage, setUsage] = useState<ProviderUsage[] | null>(null);
  const [usageError, setUsageError] = useState<string | null>(null);

  const { github, providers } = connections;
  const name = preferences.displayName.trim() || github?.name || github?.login || "You";

  const rows: AccountRow[] = [
    {
      id: "github",
      label: "GitHub",
      monogram: "GH",
      state: !github ? "checking" : !github.cliInstalled ? "missing" : github.signedIn ? "signedIn" : "signedOut",
      detail: github?.signedIn && github.login ? `@${github.login}` : null,
    },
    ...PROVIDER_ROWS.map((row): AccountRow => {
      const status = providers?.find((item) => item.provider === row.id);
      return {
        ...row,
        state: !status ? "checking" : !status.installed ? "missing" : status.loggedIn ? "signedIn" : "signedOut",
        detail: status?.loggedIn ? status.detail : null,
      };
    }),
  ];
  const connectedCount = rows.filter((row) => row.state === "signedIn").length;
  const checking = rows.some((row) => row.state === "checking");

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setUsageError(null);
    ipc
      .providerUsageSummary()
      .then((rows) => {
        if (!cancelled) setUsage(rows);
      })
      .catch((error: unknown) => {
        if (!cancelled) setUsageError(String(error));
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: MouseEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const beginSignIn = async (row: AccountRow) => {
    setSignInError(null);
    try {
      const sessionId = row.id === "github" ? await startGitHubSignIn() : await startProviderSignIn(row.id);
      setOpen(false);
      setSignIn({ sessionId, title: `Sign in to ${row.label}` });
    } catch (error) {
      setSignInError(String(error));
    }
  };

  const finishSignIn = () => {
    setSignIn(null);
    connections.refresh();
  };

  return (
    <div ref={rootRef} className="relative p-2">
      {open ? (
        <div className="absolute bottom-full left-2 z-10 mb-1 max-h-[70vh] w-72 overflow-y-auto rounded-lg border border-zinc-700 bg-zinc-900 p-1.5 shadow-2xl shadow-black/50">
          <div className={SECTION_LABEL}>Profile</div>
          <label className="block px-1 pb-1">
            <span className="sr-only">Display name</span>
            <input
              value={preferences.displayName}
              onChange={(event) => onChange({ displayName: event.target.value })}
              placeholder={github?.name ?? "Your name"}
              maxLength={60}
              className={FIELD}
            />
          </label>

          <div className={SECTION_LABEL}>Accounts</div>
          <ul className="space-y-0.5">
            {rows.map((row) => (
              <li key={row.id} className="flex items-center gap-2.5 rounded-md px-2 py-1.5">
                <span className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md bg-zinc-800 text-[10px] font-semibold text-zinc-300">
                  {row.monogram}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-xs font-medium text-zinc-100">{row.label}</span>
                  <span className="block truncate text-[11px] text-zinc-500">{statusText(row)}</span>
                </span>
                {row.state === "signedIn" ? (
                  <span className="flex shrink-0 items-center gap-1 text-[11px] text-emerald-400">
                    <span className="h-1.5 w-1.5 rounded-full bg-emerald-400" />
                    Connected
                  </span>
                ) : null}
                {row.state === "signedOut" ? (
                  <button
                    type="button"
                    onClick={() => void beginSignIn(row)}
                    className="shrink-0 rounded-md bg-indigo-600 px-2 py-1 text-[11px] font-medium text-white hover:bg-indigo-500"
                  >
                    Connect
                  </button>
                ) : null}
              </li>
            ))}
          </ul>
          {signInError ? (
            <div className="mx-1 mt-1 rounded-md border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-200">
              {signInError}
            </div>
          ) : null}
          <p className="px-2 pb-1 pt-1.5 text-[10px] leading-relaxed text-zinc-500">
            Sign-in runs each CLI&apos;s own login. Condurence never sees your tokens.
          </p>

          <div className={SECTION_LABEL}>Token usage · last {USAGE_SUMMARY_DAYS} days</div>
          <TokenUsageSection rows={usage} error={usageError} />

          <div className={SECTION_LABEL}>Appearance</div>
          <div role="radiogroup" aria-label="Appearance" className="space-y-0.5">
            {THEMES.map((theme) => {
              const selected = preferences.theme === theme.id;
              return (
                <button
                  key={theme.id}
                  type="button"
                  role="radio"
                  aria-checked={selected}
                  onClick={() => onChange({ theme: theme.id })}
                  className="flex w-full items-center justify-between rounded-md px-2 py-1.5 text-left text-xs text-zinc-200 hover:bg-zinc-800"
                >
                  {theme.label}
                  {selected ? <CheckIcon className="text-indigo-400" /> : null}
                </button>
              );
            })}
          </div>
        </div>
      ) : null}

      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
        className={`flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left transition-colors hover:bg-zinc-800/60 ${
          open ? "bg-zinc-800/60" : ""
        }`}
      >
        <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-full bg-indigo-600 text-[11px] font-semibold text-white">
          {initialsOf(name) || "Y"}
        </span>
        <span className="min-w-0 flex-1">
          <span className="block truncate text-xs font-medium text-zinc-100">{name}</span>
          <span className="block truncate text-[11px] text-zinc-500">
            {checking ? "Checking accounts…" : `${connectedCount} of ${rows.length} accounts connected`}
          </span>
        </span>
        <ChevronUpDownIcon className="shrink-0 text-zinc-500" />
      </button>

      {signIn ? <SignInDialog title={signIn.title} sessionId={signIn.sessionId} onDone={finishSignIn} /> : null}
    </div>
  );
}

function statusText(row: AccountRow): string {
  switch (row.state) {
    case "checking":
      return "Checking…";
    case "missing":
      return "CLI not installed";
    case "signedOut":
      return "Not connected";
    case "signedIn":
      return row.detail ?? "Signed in";
  }
}
