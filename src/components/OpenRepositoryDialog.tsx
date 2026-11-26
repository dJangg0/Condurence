import { useEffect, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { SignInDialog, startGitHubSignIn } from "./SignInDialog";
import { ipc } from "../lib/ipc";
import type { GitHubAccount, GitHubRepoSummary } from "../types/github";

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-xs text-zinc-100 outline-none placeholder:text-zinc-600 focus:border-indigo-500";
const BUTTON_PRIMARY =
  "rounded-md bg-indigo-600 px-2.5 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:cursor-not-allowed disabled:opacity-50";
const BUTTON_QUIET =
  "rounded-md border border-zinc-700 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800 disabled:cursor-not-allowed disabled:opacity-50";
const MUTED = "text-[11px] leading-relaxed text-zinc-500";

type Props = {
  /** Opens a checkout by path. Resolves to an error message, or `null` when the repository opened. */
  onOpenPath: (path: string) => Promise<string | null>;
  onChooseLocal: () => void;
  onClose: () => void;
};

/**
 * The Open repository dialog. It lists the signed-in account's repositories as soon as it
 * opens. Picking one clones it into `~/OpenConductor/repos` and opens it; a repository already
 * cloned there opens without downloading again. A local folder can still be chosen instead.
 */
export function OpenRepositoryDialog({ onOpenPath, onChooseLocal, onClose }: Props) {
  const [account, setAccount] = useState<GitHubAccount | null>(null);
  const [accountError, setAccountError] = useState<string | null>(null);
  const [reloadKey, setReloadKey] = useState(0);
  const [repos, setRepos] = useState<GitHubRepoSummary[] | null>(null);
  const [reposError, setReposError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [cloning, setCloning] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const [signInSession, setSignInSession] = useState<string | null>(null);

  // Fetch the account, then its repositories, each time the dialog opens or a sign-in finishes.
  useEffect(() => {
    let stale = false;
    setAccount(null);
    setAccountError(null);
    setRepos(null);
    setReposError(null);
    ipc
      .githubAccountStatus()
      .then(async (next) => {
        if (stale) return;
        setAccount(next);
        if (!next.signedIn) return;
        try {
          const list = await ipc.githubListRepositories();
          if (!stale) setRepos(list);
        } catch (error) {
          if (!stale) setReposError(String(error));
        }
      })
      .catch((error: unknown) => {
        if (!stale) setAccountError(String(error));
      });
    return () => {
      stale = true;
    };
  }, [reloadKey]);

  // Escape closes the dialog, unless a clone or a sign-in is in progress.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape" && cloning === null && signInSession === null) onClose();
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, [cloning, signInSession, onClose]);

  const openRepo = async (repo: GitHubRepoSummary) => {
    setCloning(repo.nameWithOwner);
    setOpenError(null);
    try {
      const path = await ipc.githubCloneRepository(repo.nameWithOwner);
      const failure = await onOpenPath(path);
      if (failure === null) onClose();
      else setOpenError(failure);
    } catch (error) {
      setOpenError(String(error));
    } finally {
      setCloning(null);
    }
  };

  const beginSignIn = async () => {
    setAccountError(null);
    try {
      setSignInSession(await startGitHubSignIn());
    } catch (error) {
      setAccountError(String(error));
    }
  };

  const finishSignIn = () => {
    setSignInSession(null);
    setReloadKey((n) => n + 1);
  };

  const busy = cloning !== null;
  const needle = filter.trim().toLowerCase();
  const visible = (repos ?? []).filter((repo) => repo.nameWithOwner.toLowerCase().includes(needle));

  return createPortal(
    <div
      className="fixed inset-0 z-40 flex items-center justify-center bg-black/60 p-8"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && !busy && signInSession === null) onClose();
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Open repository"
        className="flex max-h-[min(560px,80vh)] w-[min(520px,100%)] flex-col overflow-hidden rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl"
      >
        <header className="border-b border-zinc-800 px-4 py-3">
          <div className="text-sm font-semibold text-zinc-100">Open repository</div>
          <div className="text-[11px] text-zinc-500">Clone one of your GitHub repositories, or open a folder you already have.</div>
        </header>

        <div className="flex min-h-0 flex-1 flex-col gap-2 p-3">
          {accountError ? <Notice>{accountError}</Notice> : null}

          {!account && !accountError ? <p className={MUTED}>Checking GitHub…</p> : null}

          {account && !account.cliInstalled ? (
            <p className={MUTED}>
              The GitHub CLI is not installed, so your repositories cannot be listed. Install it with{" "}
              <code className="font-mono text-zinc-300">brew install gh</code>, or choose a local folder.
            </p>
          ) : null}

          {account?.cliInstalled && !account.signedIn ? (
            <div className="space-y-2">
              <p className="text-xs text-zinc-300">Sign in to GitHub to browse and clone your repositories.</p>
              <button type="button" onClick={() => void beginSignIn()} className={BUTTON_PRIMARY}>
                Sign in with GitHub
              </button>
            </div>
          ) : null}

          {account?.signedIn ? (
            <>
              <input
                value={filter}
                onChange={(event) => setFilter(event.target.value)}
                placeholder="Filter your repositories"
                aria-label="Filter your repositories"
                className={FIELD}
                disabled={busy}
                autoFocus
              />
              {reposError ? <Notice>{reposError}</Notice> : null}
              {repos === null && !reposError ? <p className={MUTED}>Loading your repositories…</p> : null}
              {repos !== null && repos.length === 0 ? <p className={MUTED}>This account has no repositories yet.</p> : null}
              {repos !== null && repos.length > 0 && visible.length === 0 ? (
                <p className={MUTED}>No repositories match.</p>
              ) : null}
              <ul className="min-h-0 flex-1 space-y-0.5 overflow-y-auto">
                {visible.map((repo) => (
                  <li key={repo.nameWithOwner}>
                    <button
                      type="button"
                      onClick={() => void openRepo(repo)}
                      disabled={busy}
                      className="w-full rounded-md px-2 py-1.5 text-left hover:bg-zinc-800 disabled:opacity-50"
                    >
                      <div className="truncate text-xs text-zinc-100">
                        {repo.nameWithOwner}
                        {cloning === repo.nameWithOwner ? <span className="text-zinc-500"> · cloning…</span> : null}
                      </div>
                      <div className="truncate text-[11px] text-zinc-500">
                        {repo.isPrivate ? "Private" : "Public"}
                        {repo.description ? ` · ${repo.description}` : ""}
                      </div>
                    </button>
                  </li>
                ))}
              </ul>
              {openError ? <Notice>{openError}</Notice> : null}
              <p className={MUTED}>Repositories are cloned into ~/OpenConductor/repos.</p>
            </>
          ) : null}
        </div>

        <footer className="flex items-center gap-2 border-t border-zinc-800 px-4 py-2.5">
          <button type="button" onClick={onChooseLocal} disabled={busy} className={BUTTON_QUIET}>
            Choose a local folder…
          </button>
          <span className="flex-1" />
          <button type="button" onClick={onClose} disabled={busy} className={BUTTON_QUIET}>
            Cancel
          </button>
        </footer>
      </div>

      {signInSession ? <SignInDialog title="Sign in to GitHub" sessionId={signInSession} onDone={finishSignIn} /> : null}
    </div>,
    document.body,
  );
}

function Notice({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-md border border-red-500/30 bg-red-500/10 px-2 py-1.5 text-[11px] text-red-200">{children}</div>
  );
}
