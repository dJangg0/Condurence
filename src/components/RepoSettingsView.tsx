import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import { DEFAULT_REPO_SETTINGS, baseBranchOf } from "../lib/repoSettings";
import type { RepoGitSettings } from "../types/orchestration";

/**
 * The repository's settings page, opened from the gear beside its name in the sidebar. Its Git
 * section follows Conductor's: where new workspaces branch from, the remote that receives pushes and
 * PRs, and what archiving and merging do. Each change is saved as soon as it is made.
 */
export function RepoSettingsView({ repoPath, repoName }: { repoPath: string; repoName: string }) {
  const [settings, setSettings] = useState<RepoGitSettings | null>(null);
  const [remotes, setRemotes] = useState<string[]>([]);
  const [branches, setBranches] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    Promise.all([ipc.getRepoGitSettings(repoPath), ipc.listRemotes(repoPath)])
      .then(([loaded, names]) => {
        if (cancelled) return;
        setSettings(loaded);
        setRemotes(names);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath]);

  // The base choices are the branches of the chosen remote, refreshed when the remote changes.
  const remote = settings?.remote ?? DEFAULT_REPO_SETTINGS.remote;
  useEffect(() => {
    let cancelled = false;
    ipc
      .listRemoteBranches(repoPath, remote)
      .then((list) => {
        if (!cancelled) setBranches(list);
      })
      .catch(() => {
        if (!cancelled) setBranches([]);
      });
    return () => {
      cancelled = true;
    };
  }, [repoPath, remote]);

  const save = async (next: RepoGitSettings) => {
    setSettings(next);
    setSaving(true);
    try {
      await ipc.saveRepoGitSettings(repoPath, next);
      setError(null);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setSaving(false);
    }
  };

  const changeRemote = (nextRemote: string) => {
    if (!settings) return;
    // Keep the base's branch name and move it to the new remote.
    void save({ ...settings, remote: nextRemote, baseRef: `${nextRemote}/${baseBranchOf(settings)}` });
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto max-w-3xl px-8 py-8">
        <div className="text-xs text-zinc-500">{repoName}</div>
        <h1 className="mt-1 text-2xl font-semibold tracking-tight text-zinc-100">Repository settings</h1>

        {settings === null ? (
          <p className="mt-6 text-sm text-zinc-500">{error ?? "Loading…"}</p>
        ) : (
          <GitSettingsPanel
            settings={settings}
            remotes={remotes}
            branches={branches}
            onChange={(patch) => void save({ ...settings, ...patch })}
            onRemoteChange={changeRemote}
            error={error}
            saving={saving}
          />
        )}
      </div>
    </div>
  );
}

/** The Git section itself: pure, so it can be rendered without a repository. */
export function GitSettingsPanel({
  settings,
  remotes,
  branches,
  onChange,
  onRemoteChange,
  error,
  saving,
}: {
  settings: RepoGitSettings;
  remotes: string[];
  branches: string[];
  onChange: (patch: Partial<RepoGitSettings>) => void;
  onRemoteChange: (remote: string) => void;
  error: string | null;
  saving: boolean;
}) {
  // The current base is always offered, even before the remote's branches have loaded.
  const baseOptions = branches.includes(settings.baseRef) ? branches : [settings.baseRef, ...branches];
  const remoteOptions = remotes.includes(settings.remote) ? remotes : [settings.remote, ...remotes];

  return (
    <section className="mt-6">
      <h2 className="text-lg font-medium text-zinc-100">Git</h2>

      <div className="mt-6 space-y-2">
        <div className="text-sm font-medium text-zinc-100">Branch new workspaces from</div>
        <p className="text-xs text-zinc-500">Each workspace is an isolated copy of your codebase.</p>
        <select
          aria-label="Branch new workspaces from"
          value={settings.baseRef}
          onChange={(event) => onChange({ baseRef: event.target.value })}
          className="mt-2 rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-sm text-zinc-100"
        >
          {baseOptions.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </div>

      <div className="mt-8 space-y-2 border-t border-zinc-800 pt-8">
        <div className="text-sm font-medium text-zinc-100">Remote origin</div>
        <p className="text-xs text-zinc-500">Where should we push, pull, and create PRs?</p>
        <select
          aria-label="Remote origin"
          value={settings.remote}
          onChange={(event) => onRemoteChange(event.target.value)}
          className="mt-2 rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-sm text-zinc-100"
        >
          {remoteOptions.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      </div>

      <ToggleRow
        label="Delete branch on archive"
        description="Delete the local branch when archiving a workspace. To delete the remote branch, configure it on GitHub."
        checked={settings.deleteBranchOnArchive}
        onChange={(value) => onChange({ deleteBranchOnArchive: value })}
      />
      <ToggleRow
        label="Archive on merge"
        description="Automatically archive a workspace after merging its PR in Condurence."
        checked={settings.archiveOnMerge}
        onChange={(value) => onChange({ archiveOnMerge: value })}
      />
      <ToggleRow
        label="Set upstream on plain git push"
        description="Configure new Condurence workspaces so plain git push sets a branch upstream. Turning this off avoids writing Git worktree config, but PR info may be less reliable until branches have an upstream."
        checked={settings.setUpstreamOnPush}
        onChange={(value) => onChange({ setUpstreamOnPush: value })}
      />

      <div className="mt-6 min-h-4 text-xs">
        {error ? <span className="text-red-300">{error}</span> : saving ? <span className="text-zinc-500">Saving…</span> : null}
      </div>
    </section>
  );
}

/** The easing curve iOS uses for its switches, so the motion reads as familiar. */
const IOS_EASE = "ease-[cubic-bezier(0.32,0.72,0,1)]";

function ToggleRow({
  label,
  description,
  checked,
  onChange,
}: {
  label: string;
  description: string;
  checked: boolean;
  onChange: (value: boolean) => void;
}) {
  return (
    <div className="mt-8 flex items-start justify-between gap-6 border-t border-zinc-800 pt-8">
      <div className="min-w-0">
        <div className="text-sm font-medium text-zinc-100">{label}</div>
        <p className="mt-2 text-xs leading-relaxed text-zinc-500">{description}</p>
      </div>
      {/* An iPhone-style switch: the track and knob share one slow ease-out, and the knob
          stretches a little while pressed. Reduced-motion users get the instant change. */}
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        onClick={() => onChange(!checked)}
        className={`group relative mt-0.5 h-6 w-11 shrink-0 cursor-pointer select-none rounded-full transition-colors duration-300 ${IOS_EASE} motion-reduce:transition-none focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-emerald-400/60 ${
          checked ? "bg-[#34c759]" : "bg-zinc-700"
        }`}
      >
        <span
          className={`absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white shadow-[0_1px_3px_rgba(0,0,0,0.35)] transition-transform duration-300 ${IOS_EASE} motion-reduce:transition-none group-active:scale-x-[1.15] ${
            checked ? "translate-x-5" : "translate-x-0"
          }`}
        />
      </button>
    </div>
  );
}
