import { useEffect, useRef, useState, type ReactNode } from "react";
import { SignInDialog, startGitHubSignIn } from "./SignInDialog";
import { ipc } from "../lib/ipc";
import type { GitHubAccount, GitHubRepoConnection, GitHubRepoSummary } from "../types/github";

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500";
const BUTTON_PRIMARY =
  "rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50";
const BUTTON_QUIET =
  "rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50";
const SECTION_LABEL = "pb-1.5 text-[11px] font-semibold uppercase tracking-wider text-zinc-500";
const MUTED = "text-[11px] leading-relaxed text-zinc-500";

type Tone = "ok" | "action" | "muted";

const TONE_DOT: Record<Tone, string> = {
  ok: "bg-emerald-400",
  action: "bg-amber-400",
  muted: "bg-zinc-500",
};

/**
 * GitHub status for the open repository, in the header. The chip shows the repository link
 * state; the popover shows the signed-in account and lets the user link a repository.
 */
export function GitHubConnection({ repoPath }: { repoPath: string | null }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  const [account, setAccount] = useState<GitHubAccount | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [accountRevision, setAccountRevision] = useState(0);

  const [connection, setConnection] = useState<GitHubRepoConnection | null>(null);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [connectionRevision, setConnectionRevision] = useState(0);

  const [picking, setPicking] = useState(false);
  const [repos, setRepos] = useState<GitHubRepoSummary[] | null>(null);
  const [reposError, setReposError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [linking, setLinking] = useState<string | null>(null);
  const [linkError, setLinkError] = useState<string | null>(null);

  const [signInSession, setSignInSession] = useState<string | null>(null);

  // Account state: loaded on mount and again after a sign-in.
  useEffect(() => {
    let stale = false;
    ipc
      .githubAccountStatus()
      .then((next) => {
        if (stale) return;
        setAccount(next);
        setAccountError(null);
      })
      .catch((error: unknown) => {
        if (!stale) setAccountError(String(error));
      });
    return () => {
      stale = true;
    };
  }, [accountRevision]);

  // Repository link state: re-checked when the open repository changes or a sign-in finishes.
  useEffect(() => {
    setPicking(false);
    setLinkError(null);
    setConnection(null);
    setConnectionError(null);
    if (!repoPath) return;
    let stale = false;
    ipc
      .githubRepoConnection(repoPath)
      .then((next) => {
        if (stale) return;
        setConnection(next);
      })
      .catch((error: unknown) => {
        if (!stale) setConnectionError(String(error));
      });
    return () => {
      stale = true;
    };
  }, [repoPath, connectionRevision]);

  // Close the popover on an outside click or Escape.
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

  const loadRepos = async () => {
    setReposError(null);
    try {
      setRepos(await ipc.githubListRepositories());
    } catch (error) {
      setReposError(String(error));
    }
  };

  const openPicker = () => {
    setPicking(true);
    setLinkError(null);
    setFilter("");
    if (repos === null) void loadRepos();
  };

  const linkRepository = async (repo: GitHubRepoSummary) => {
    if (!repoPath) return;
    setLinking(repo.nameWithOwner);
    setLinkError(null);
    try {
      const next = await ipc.githubConnectRepository(repoPath, repo.nameWithOwner);
      setConnection(next);
      setPicking(false);
    } catch (error) {
      setLinkError(String(error));
    } finally {
      setLinking(null);
    }
  };

  const startSignIn = async () => {
    try {
      const sessionId = await startGitHubSignIn();
      setOpen(false);
      setSignInSession(sessionId);
    } catch (error) {
      setAccountError(String(error));
    }
  };

  const finishSignIn = () => {
    setSignInSession(null);
    setRepos(null);
    setAccountRevision((n) => n + 1);
    setConnectionRevision((n) => n + 1);
  };

  const summary = summarize(account, repoPath !== null, connection, connectionError);

  return (
    <div ref={rootRef} className="relative ml-auto">
      <button
        type="button"
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((current) => !current)}
        className={`flex max-w-xs items-center gap-2 rounded-md border px-2.5 py-1.5 text-xs transition-colors ${
          open ? "border-zinc-600 bg-zinc-800" : "border-zinc-800 bg-zinc-950 hover:bg-zinc-800/60"
        }`}
      >
        <span className={`h-1.5 w-1.5 shrink-0 rounded-full ${TONE_DOT[summary.tone]}`} />
        <span className="shrink-0 text-zinc-500">GitHub</span>
        <span className="min-w-0 truncate font-medium text-zinc-200">{summary.label}</span>
      </button>

      {open ? (
        <div
          role="dialog"
          aria-label="GitHub connection"
          className="absolute right-0 top-full z-30 mt-1.5 w-80 rounded-lg border border-zinc-700 bg-zinc-900 p-3 shadow-2xl shadow-black/50"
        >
          <AccountSection
            account={account}
            error={accountError}
            onSignIn={() => void startSignIn()}
            onRefresh={() => setAccountRevision((n) => n + 1)}
          />
          {repoPath ? (
            <RepositorySection
              account={account}
              connection={connection}
              error={connectionError}
              picking={picking}
              repos={repos}
              reposError={reposError}
              filter={filter}
              onFilterChange={setFilter}
              linking={linking}
              linkError={linkError}
              onRetry={() => setConnectionRevision((n) => n + 1)}
              onOpenPicker={openPicker}
              onClosePicker={() => setPicking(false)}
              onLink={(repo) => void linkRepository(repo)}
              onSignIn={() => void startSignIn()}
            />
          ) : null}
        </div>
      ) : null}

      {signInSession ? <SignInDialog title="Sign in to GitHub" sessionId={signInSession} onDone={finishSignIn} /> : null}
    </div>
  );
}

/** Picks the header chip's label and status colour from the account and repository state. */
function summarize(
  account: GitHubAccount | null,
  repoOpen: boolean,
  connection: GitHubRepoConnection | null,
  connectionError: string | null,
): { label: string; tone: Tone } {
  if (repoOpen && (connection || connectionError)) {
    if (connectionError) return { label: "Repository error", tone: "action" };
    switch (connection?.state) {
      case "linked":
        return { label: connection.nameWithOwner ?? "Linked", tone: "ok" };
      case "unverified":
        return { label: "Access not verified", tone: "action" };
      case "otherRemote":
        return { label: "Non-GitHub remote", tone: "muted" };
      case "unlinked":
        return account?.signedIn === false
          ? { label: "Sign in to GitHub", tone: "action" }
          : { label: "Connect repository", tone: "action" };
      default:
        break;
    }
  }
  if (!account) return { label: "Checking…", tone: "muted" };
  if (!account.cliInstalled) return { label: "GitHub CLI not installed", tone: "muted" };
  if (!account.signedIn) return { label: "Sign in", tone: "action" };
  return { label: account.login ? `@${account.login}` : "Signed in", tone: "ok" };
}

function AccountSection({
  account,
  error,
  onSignIn,
  onRefresh,
}: {
  account: GitHubAccount | null;
  error: string | null;
  onSignIn: () => void;
  onRefresh: () => void;
}) {
  return (
    <section>
      <div className={SECTION_LABEL}>Account</div>
      {error ? <Notice>{error}</Notice> : null}
      {!account ? (
        <p className={MUTED}>Checking GitHub…</p>
      ) : !account.cliInstalled ? (
        <div className="space-y-2">
          <p className="text-xs text-zinc-300">
            The GitHub CLI (<code className="font-mono text-zinc-200">gh</code>) was not found on your PATH.
          </p>
          <p className={MUTED}>
            Install it with <code className="font-mono text-zinc-300">brew install gh</code>, then refresh.
          </p>
          <button type="button" onClick={onRefresh} className={BUTTON_QUIET}>
            Refresh
          </button>
        </div>
      ) : !account.signedIn ? (
        <div className="space-y-2">
          <p className="text-xs text-zinc-300">You are not signed in to github.com.</p>
          <button type="button" onClick={onSignIn} className={BUTTON_PRIMARY}>
            Sign in with GitHub
          </button>
          <p className={MUTED}>
            Opens <code className="font-mono">gh auth login</code> in a terminal. Condurence never sees your token.
          </p>
        </div>
      ) : (
        <div className="flex items-center gap-2.5">
          <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-indigo-600 text-[11px] font-semibold text-white">
            {initialsOf(account.name ?? account.login ?? "GH")}
          </span>
          <div className="min-w-0">
            <div className="truncate text-xs font-medium text-zinc-100">{account.name ?? account.login}</div>
            <div className="truncate text-[11px] text-zinc-500">@{account.login} · github.com</div>
          </div>
        </div>
      )}
    </section>
  );
}

function RepositorySection({
  account,
  connection,
  error,
  picking,
  repos,
  reposError,
  filter,
  onFilterChange,
  linking,
  linkError,
  onRetry,
  onOpenPicker,
  onClosePicker,
  onLink,
  onSignIn,
}: {
  account: GitHubAccount | null;
  connection: GitHubRepoConnection | null;
  error: string | null;
  picking: boolean;
  repos: GitHubRepoSummary[] | null;
  reposError: string | null;
  filter: string;
  onFilterChange: (value: string) => void;
  linking: string | null;
  linkError: string | null;
  onRetry: () => void;
  onOpenPicker: () => void;
  onClosePicker: () => void;
  onLink: (repo: GitHubRepoSummary) => void;
  onSignIn: () => void;
}) {
  return (
    <section className="mt-3 border-t border-zinc-800 pt-3">
      <div className={SECTION_LABEL}>Repository</div>
      {error ? <Notice>{error}</Notice> : null}
      {!connection && !error ? <p className={MUTED}>Checking repository…</p> : null}

      {connection?.state === "linked" ? (
        <div className="space-y-1">
          <div className="flex min-w-0 items-center gap-2">
            <span className="truncate text-xs font-medium text-zinc-100">{connection.nameWithOwner}</span>
            <span className="shrink-0 rounded bg-zinc-800 px-1.5 py-0.5 text-[10px] text-zinc-300">
              {connection.isPrivate ? "Private" : "Public"}
            </span>
          </div>
          <p className={MUTED}>
            {[
              connection.viewerPermission ? `Your access: ${connection.viewerPermission.toLowerCase()}` : null,
              connection.defaultBranch ? `default branch ${connection.defaultBranch}` : null,
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
          {connection.remoteUrl ? (
            <p className="truncate font-mono text-[10px] text-zinc-500" title={connection.remoteUrl}>
              origin: {connection.remoteUrl}
            </p>
          ) : null}
        </div>
      ) : null}

      {connection?.state === "unverified" ? (
        <div className="space-y-2">
          <p className="text-xs text-zinc-300">The origin remote is on GitHub, but its access could not be verified.</p>
          {connection.detail ? <Notice>{connection.detail}</Notice> : null}
          {connection.remoteUrl ? (
            <p className="truncate font-mono text-[10px] text-zinc-500" title={connection.remoteUrl}>
              origin: {connection.remoteUrl}
            </p>
          ) : null}
          <button type="button" onClick={onRetry} className={BUTTON_QUIET}>
            Check again
          </button>
        </div>
      ) : null}

      {connection?.state === "otherRemote" ? (
        <div className="space-y-2">
          <p className="text-xs text-zinc-300">This repository&apos;s origin is not on GitHub.</p>
          <p className={MUTED}>
            origin: <span className="font-mono text-zinc-400">{connection.remoteUrl}</span>. Remove or rename that remote to
            link a GitHub repository here.
          </p>
        </div>
      ) : null}

      {connection?.state === "unlinked" ? (
        <div className="space-y-2">
          <p className="text-xs text-zinc-300">This repository has no GitHub remote yet.</p>
          {!picking ? (
            <button type="button" onClick={onOpenPicker} className={BUTTON_PRIMARY}>
              Connect repository…
            </button>
          ) : (
            <RepoPicker
              account={account}
              repos={repos}
              reposError={reposError}
              filter={filter}
              onFilterChange={onFilterChange}
              linking={linking}
              linkError={linkError}
              onClose={onClosePicker}
              onLink={onLink}
              onSignIn={onSignIn}
            />
          )}
        </div>
      ) : null}
    </section>
  );
}

function RepoPicker({
  account,
  repos,
  reposError,
  filter,
  onFilterChange,
  linking,
  linkError,
  onClose,
  onLink,
  onSignIn,
}: {
  account: GitHubAccount | null;
  repos: GitHubRepoSummary[] | null;
  reposError: string | null;
  filter: string;
  onFilterChange: (value: string) => void;
  linking: string | null;
  linkError: string | null;
  onClose: () => void;
  onLink: (repo: GitHubRepoSummary) => void;
  onSignIn: () => void;
}) {
  if (!account?.signedIn) {
    return (
      <div className="space-y-2">
        <p className={MUTED}>Sign in to GitHub to choose one of your repositories.</p>
        <button type="button" onClick={onSignIn} className={BUTTON_PRIMARY}>
          Sign in with GitHub
        </button>
      </div>
    );
  }

  const needle = filter.trim().toLowerCase();
  const visible = (repos ?? []).filter((repo) => repo.nameWithOwner.toLowerCase().includes(needle));

  return (
    <div className="space-y-2">
      <input
        value={filter}
        onChange={(event) => onFilterChange(event.target.value)}
        placeholder="Filter your repositories"
        aria-label="Filter your repositories"
        className={FIELD}
        autoFocus
      />
      {reposError ? <Notice>{reposError}</Notice> : null}
      {repos === null && !reposError ? <p className={MUTED}>Loading your repositories…</p> : null}
      {repos !== null && visible.length === 0 ? <p className={MUTED}>No repositories match.</p> : null}
      <ul className="max-h-56 space-y-0.5 overflow-y-auto">
        {visible.map((repo) => (
          <li key={repo.nameWithOwner}>
            <button
              type="button"
              onClick={() => onLink(repo)}
              disabled={linking !== null}
              className="w-full rounded-md px-2 py-1.5 text-left hover:bg-zinc-800 disabled:opacity-50"
            >
              <div className="truncate text-xs text-zinc-100">
                {repo.nameWithOwner}
                {linking === repo.nameWithOwner ? <span className="text-zinc-500"> · linking…</span> : null}
              </div>
              <div className="truncate text-[11px] text-zinc-500">
                {repo.isPrivate ? "Private" : "Public"}
                {repo.description ? ` · ${repo.description}` : ""}
              </div>
            </button>
          </li>
        ))}
      </ul>
      {linkError ? <Notice>{linkError}</Notice> : null}
      <button type="button" onClick={onClose} className={BUTTON_QUIET} disabled={linking !== null}>
        Cancel
      </button>
    </div>
  );
}

function Notice({ children }: { children: ReactNode }) {
  return (
    <div className="mb-2 rounded-md border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-200">
      {children}
    </div>
  );
}

function initialsOf(name: string): string {
  return (
    name
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((part) => part[0])
      .join("")
      .toUpperCase() || "GH"
  );
}
