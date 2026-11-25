import { useState } from "react";
import {
  ArchiveIcon,
  ChevronDownIcon,
  ChevronRightIcon,
  GearIcon,
  OrchestrationIcon,
  PlusIcon,
  RestoreIcon,
  TrashIcon,
  WorkspacesIcon,
} from "./icons";
import { ProfileMenu } from "./ProfileMenu";
import type { Connections } from "../lib/connections";
import type { Preferences } from "../lib/preferences";
import type { RepoRecord, WorktreeInfo } from "../types/orchestration";

export type ViewMode = "workspaces" | "orchestration";

const VIEWS: Array<{ id: ViewMode; label: string; hint: string; Icon: typeof WorkspacesIcon }> = [
  { id: "workspaces", label: "Workspaces", hint: "Isolated worktrees, diffs and terminals", Icon: WorkspacesIcon },
  {
    id: "orchestration",
    label: "Agent Orchestration",
    hint: "Railway-style multi-agent topology",
    Icon: OrchestrationIcon,
  },
];

type SidebarProps = {
  mode: ViewMode;
  onModeChange: (mode: ViewMode) => void;
  /** Path of the open repository, or `null`. */
  activePath: string | null;
  /** The selected worktree of the open repository, or `null` for its main checkout. */
  activeWorktreePath: string | null;
  recent: RepoRecord[];
  archived: RepoRecord[];
  /** Workspaces per repository path. A repository missing here has not loaded yet. */
  worktrees: Record<string, WorktreeInfo[]>;
  onOpenRepository: () => void;
  onSelectWorktree: (repoPath: string, worktreePath: string) => void;
  onCreateWorkspace: (repoPath: string) => void;
  onArchive: (path: string, archived: boolean) => void;
  onRemove: (path: string) => void;
  settingsActive: boolean;
  onOpenSettings: () => void;
  connections: Connections;
  preferences: Preferences;
  onPreferencesChange: (patch: Partial<Preferences>) => void;
};

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

/**
 * App sidebar in the Conductor pattern: views and Open repository on top, then each repository
 * as a group with its workspaces nested beneath and a "+" to create another. The account menu
 * and settings gear sit at the bottom.
 */
export function Sidebar({
  mode,
  onModeChange,
  activePath,
  activeWorktreePath,
  recent,
  archived,
  worktrees,
  onOpenRepository,
  onSelectWorktree,
  onCreateWorkspace,
  onArchive,
  onRemove,
  settingsActive,
  onOpenSettings,
  connections,
  preferences,
  onPreferencesChange,
}: SidebarProps) {
  const [showArchived, setShowArchived] = useState(false);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  // Remove asks for a second click on the same row before it forgets the entry.
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  const toggle = (path: string) =>
    setCollapsed((current) => {
      const next = new Set(current);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const group = (item: RepoRecord) => {
    const list = worktrees[item.path] ?? [];
    const isOpen = activePath === item.path;
    const effective = isOpen ? (activeWorktreePath ?? list.find((wt) => wt.isMain)?.path ?? null) : null;
    const expanded = !collapsed.has(item.path) && !item.archived;

    return (
      <li key={item.path}>
        <div className="group relative flex items-center">
          <button
            type="button"
            onClick={() => (item.archived ? undefined : toggle(item.path))}
            aria-expanded={item.archived ? undefined : expanded}
            title={item.path}
            className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md py-1.5 pl-1.5 pr-1 text-left hover:bg-zinc-800/60"
          >
            {item.archived ? null : expanded ? (
              <ChevronDownIcon className="shrink-0 text-zinc-500" />
            ) : (
              <ChevronRightIcon className="shrink-0 text-zinc-500" />
            )}
            <span className={`truncate text-xs font-semibold ${item.archived ? "text-zinc-500" : "text-zinc-200"}`}>
              {item.name}
            </span>
          </button>

          <div className="flex shrink-0 items-center gap-0.5 pr-1">
            {confirmRemove === item.path ? (
              <button
                type="button"
                onClick={() => {
                  setConfirmRemove(null);
                  onRemove(item.path);
                }}
                onBlur={() => setConfirmRemove(null)}
                autoFocus
                title="Remove from the list. Files on disk are not deleted."
                className="rounded bg-red-600 px-1.5 py-0.5 text-[10px] font-medium text-white hover:bg-red-500"
              >
                Remove?
              </button>
            ) : (
              <>
                <span className="flex items-center gap-0.5 opacity-0 focus-within:opacity-100 group-hover:opacity-100">
                  <button
                    type="button"
                    onClick={() => onArchive(item.path, !item.archived)}
                    title={item.archived ? "Restore" : "Archive"}
                    aria-label={`${item.archived ? "Restore" : "Archive"} ${item.name}`}
                    className="rounded p-1 text-zinc-500 hover:bg-zinc-700 hover:text-zinc-200"
                  >
                    {item.archived ? <RestoreIcon /> : <ArchiveIcon />}
                  </button>
                  <button
                    type="button"
                    onClick={() => setConfirmRemove(item.path)}
                    title="Remove from list"
                    aria-label={`Remove ${item.name} from list`}
                    className="rounded p-1 text-zinc-500 hover:bg-zinc-700 hover:text-red-300"
                  >
                    <TrashIcon />
                  </button>
                </span>
                {item.archived ? null : (
                  <button
                    type="button"
                    onClick={() => onCreateWorkspace(item.path)}
                    title="Create workspace"
                    aria-label={`Create workspace in ${item.name}`}
                    className="rounded p-1 text-zinc-400 hover:bg-zinc-700 hover:text-zinc-100"
                  >
                    <PlusIcon />
                  </button>
                )}
              </>
            )}
          </div>
        </div>

        {expanded ? (
          <ul className="space-y-0.5 pb-1.5 pt-0.5">
            {list.map((worktree) => {
              const selected = !settingsActive && effective === worktree.path;
              const label = worktree.branch ?? `detached ${worktree.head.slice(0, 8)}`;
              return (
                <li key={worktree.path}>
                  <button
                    type="button"
                    onClick={() => onSelectWorktree(item.path, worktree.path)}
                    title={worktree.path}
                    className={`flex w-full items-center gap-2.5 rounded-md py-1.5 pl-6 pr-2 text-left text-xs ${
                      selected ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/60 hover:text-zinc-200"
                    }`}
                  >
                    <span
                      className={`h-2 w-2 shrink-0 rounded-sm ${worktree.isMain ? "bg-zinc-600" : "bg-indigo-400"}`}
                      aria-hidden
                    />
                    <span className="min-w-0 flex-1 truncate">{label}</span>
                    {worktree.isMain ? <span className="shrink-0 text-[10px] text-zinc-600">root</span> : null}
                  </button>
                </li>
              );
            })}
            {list.length === 0 && worktrees[item.path] !== undefined ? (
              <li className="py-1 pl-6 text-[11px] text-zinc-600">{basename(item.path)} could not be read.</li>
            ) : null}
          </ul>
        ) : null}
      </li>
    );
  };

  return (
    <aside className="flex w-60 shrink-0 flex-col border-r border-zinc-800 bg-zinc-900/60">
      <nav className="space-y-0.5 px-2 pb-2 pt-3" aria-label="Views">
        {VIEWS.map(({ id, label, hint, Icon }) => {
          const active = !settingsActive && mode === id;
          return (
            <button
              key={id}
              type="button"
              title={hint}
              aria-current={active ? "page" : undefined}
              onClick={() => onModeChange(id)}
              className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium transition-colors ${
                active ? "bg-zinc-800 text-zinc-100" : "text-zinc-400 hover:bg-zinc-800/60 hover:text-zinc-200"
              }`}
            >
              <Icon className="shrink-0" />
              <span className="truncate">{label}</span>
            </button>
          );
        })}
        <button
          type="button"
          onClick={onOpenRepository}
          className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium text-zinc-400 transition-colors hover:bg-zinc-800/60 hover:text-zinc-200"
        >
          <PlusIcon className="shrink-0" />
          <span className="truncate">Open repository…</span>
        </button>
      </nav>

      <div className="min-h-0 flex-1 overflow-y-auto border-t border-zinc-800 px-2 pb-3 pt-3">
        <div className="px-2 pb-1.5 text-[11px] font-semibold text-zinc-500">My repositories</div>
        {recent.length === 0 ? <div className="px-2 text-xs text-zinc-600">No repositories yet.</div> : null}
        <ul className="space-y-0.5">{recent.map(group)}</ul>

        {archived.length > 0 ? (
          <div className="mt-3">
            <button
              type="button"
              aria-expanded={showArchived}
              onClick={() => setShowArchived((current) => !current)}
              className="flex w-full items-center gap-1.5 px-2 pb-1.5 text-left text-[11px] font-semibold text-zinc-500 hover:text-zinc-300"
            >
              {showArchived ? <ChevronDownIcon /> : <ChevronRightIcon />}
              Archived · {archived.length}
            </button>
            {showArchived ? <ul className="space-y-0.5">{archived.map(group)}</ul> : null}
          </div>
        ) : null}
      </div>

      <div className="flex items-center border-t border-zinc-800">
        <div className="min-w-0 flex-1">
          <ProfileMenu preferences={preferences} onChange={onPreferencesChange} connections={connections} />
        </div>
        <button
          type="button"
          onClick={onOpenSettings}
          title="Settings"
          aria-label="Settings"
          aria-current={settingsActive ? "page" : undefined}
          className={`mr-2 shrink-0 rounded-md p-2 hover:bg-zinc-800/60 hover:text-zinc-200 ${
            settingsActive ? "bg-zinc-800 text-zinc-100" : "text-zinc-500"
          }`}
        >
          <GearIcon />
        </button>
      </div>
    </aside>
  );
}
