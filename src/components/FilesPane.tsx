import { useEffect, useMemo, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import { ancestorDirs, baseName, dirsHolding, matchFiles, treeRows } from "../lib/fileTree";
import { STATUS_BADGE } from "./DiffViewer";
import { ChevronDownIcon, ChevronRightIcon, FileIcon, FolderIcon, SearchIcon } from "./icons";
import type { FileContent, FileListing, FileStatus } from "../types/orchestration";

type Props = {
  worktreePath: string;
  /** Files the workspace changes, by path. They carry a badge in the tree and a way to their diff. */
  changed: ReadonlyMap<string, FileStatus>;
  /** Moves when the workspace's files may have changed, so the tree and the open file are read again. */
  version: number;
  onShowDiff: (path: string) => void;
};

/**
 * Every file of a workspace, as a tree with a search box and a viewer. What it shows is what git
 * lists for the workspace: tracked files and untracked ones that are not ignored.
 */
export function FilesPane({ worktreePath, changed, version, onShowDiff }: Props) {
  const [listing, setListing] = useState<FileListing | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState<ReadonlySet<string>>(new Set());
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [content, setContent] = useState<FileContent | null>(null);
  const [contentError, setContentError] = useState<string | null>(null);
  // Answers that arrive after the user moved on are dropped.
  const generation = useRef(0);

  // A different workspace starts from a closed tree.
  useEffect(() => {
    setOpen(new Set());
    setQuery("");
    setSelected(null);
    setContent(null);
    setContentError(null);
    setListing(null);
  }, [worktreePath]);

  useEffect(() => {
    let cancelled = false;
    ipc
      .listWorktreeFiles(worktreePath)
      .then((next) => {
        if (cancelled) return;
        setListing(next);
        setError(null);
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [worktreePath, version]);

  // The open file is read when picked and again when the workspace changes under it.
  useEffect(() => {
    if (!selected) {
      setContent(null);
      return;
    }
    const mine = ++generation.current;
    ipc
      .readWorktreeFile(worktreePath, selected)
      .then((next) => {
        if (generation.current !== mine) return;
        setContent(next);
        setContentError(null);
      })
      .catch((reason: unknown) => {
        if (generation.current !== mine) return;
        setContent(null);
        setContentError(String(reason));
      });
  }, [worktreePath, selected, version]);

  const files = listing?.files ?? [];
  const searching = query.trim().length > 0;
  const rows = useMemo(
    () => (searching ? [] : treeRows(files, open)),
    [files, open, searching],
  );
  const matches = useMemo(() => (searching ? matchFiles(files, query) : []), [files, query, searching]);
  const changedDirs = useMemo(() => dirsHolding(changed.keys()), [changed]);

  const toggle = (dir: string) =>
    setOpen((current) => {
      const next = new Set(current);
      if (!next.delete(dir)) next.add(dir);
      return next;
    });

  const pick = (path: string) => {
    setSelected(path);
    setContentError(null);
    // Reveal it in the tree, so clearing a search lands on the file.
    setOpen((current) => new Set([...current, ...ancestorDirs(path)]));
  };

  const status = selected ? changed.get(selected) : undefined;

  return (
    <div className="flex min-h-0 flex-1">
      <div className="flex w-72 shrink-0 flex-col border-r border-zinc-800">
        <label className="flex items-center gap-2 border-b border-zinc-800 px-3 py-2 text-zinc-500 focus-within:text-zinc-300">
          <SearchIcon />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Find a file"
            aria-label="Find a file"
            className="min-w-0 flex-1 bg-transparent text-xs text-zinc-100 outline-none placeholder:text-zinc-600"
          />
        </label>
        <div className="min-h-0 flex-1 overflow-y-auto py-1">
          {error ? (
            <div className="p-4 text-xs text-red-300">{error}</div>
          ) : listing === null ? (
            <div className="p-4 text-xs text-zinc-500">Reading files…</div>
          ) : files.length === 0 ? (
            <div className="p-4 text-xs text-zinc-500">This workspace has no files.</div>
          ) : searching ? (
            matches.length === 0 ? (
              <div className="p-4 text-xs text-zinc-500">No file matches “{query.trim()}”.</div>
            ) : (
              <ul>
                {matches.map((path) => (
                  <li key={path}>
                    <FileRow
                      label={baseName(path)}
                      detail={path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : undefined}
                      depth={0}
                      active={path === selected}
                      status={changed.get(path)}
                      onClick={() => pick(path)}
                    />
                  </li>
                ))}
              </ul>
            )
          ) : (
            <ul role="tree" aria-label="Files">
              {rows.map((row) => (
                <li key={`${row.kind}:${row.path}`} role="treeitem" aria-expanded={row.kind === "dir" ? row.open : undefined}>
                  {row.kind === "dir" ? (
                    <button
                      type="button"
                      onClick={() => toggle(row.path)}
                      style={{ paddingLeft: 8 + row.depth * 14 }}
                      className="flex w-full items-center gap-1.5 py-1 pr-2 text-left text-xs text-zinc-300 hover:bg-zinc-800/60"
                    >
                      {row.open ? <ChevronDownIcon className="shrink-0 text-zinc-500" /> : <ChevronRightIcon className="shrink-0 text-zinc-500" />}
                      <FolderIcon open={row.open} className="shrink-0 text-zinc-500" />
                      <span className="min-w-0 flex-1 truncate">{row.name}</span>
                      {changedDirs.has(row.path) ? <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-amber-400" title="Holds changed files" /> : null}
                    </button>
                  ) : (
                    <FileRow
                      label={row.name}
                      depth={row.depth + 1}
                      active={row.path === selected}
                      status={changed.get(row.path)}
                      onClick={() => pick(row.path)}
                    />
                  )}
                </li>
              ))}
            </ul>
          )}
          {listing?.truncated ? (
            <div className="border-t border-zinc-800 p-3 text-[11px] text-zinc-500">
              Only the first {files.length.toLocaleString()} files are listed. Search narrows them down.
            </div>
          ) : null}
        </div>
      </div>

      <div className="flex min-w-0 flex-1 flex-col bg-zinc-950">
        {selected ? (
          <>
            <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-2 text-xs">
              <span className="min-w-0 flex-1 truncate font-mono text-zinc-200" title={selected}>
                {selected}
              </span>
              {content ? <span className="shrink-0 text-[11px] text-zinc-500">{formatSize(content.size)}</span> : null}
              {status ? (
                <button type="button" onClick={() => onShowDiff(selected)} className="shrink-0 text-[11px] text-indigo-300 hover:text-indigo-200">
                  View changes
                </button>
              ) : null}
            </div>
            <div className="min-h-0 flex-1 overflow-auto">
              <FileText content={content} error={contentError} />
            </div>
          </>
        ) : (
          <div className="p-6 text-sm text-zinc-500">Select a file to read it.</div>
        )}
      </div>
    </div>
  );
}

function FileRow({
  label,
  detail,
  depth,
  active,
  status,
  onClick,
}: {
  label: string;
  /** The folder, when the row is a search hit and has no tree around it. */
  detail?: string;
  depth: number;
  active: boolean;
  status: FileStatus | undefined;
  onClick: () => void;
}) {
  const badge = status ? STATUS_BADGE[status] : null;
  return (
    <button
      type="button"
      onClick={onClick}
      aria-current={active ? "true" : undefined}
      style={{ paddingLeft: 8 + depth * 14 }}
      className={`flex w-full items-center gap-1.5 py-1 pr-2 text-left text-xs ${
        active ? "bg-zinc-800 text-zinc-100" : "text-zinc-300 hover:bg-zinc-800/60"
      }`}
    >
      <FileIcon className="shrink-0 text-zinc-600" />
      <span className="min-w-0 flex-1 truncate" title={detail ? `${detail}/${label}` : label}>
        {label}
        {detail ? <span className="ml-1.5 text-[10px] text-zinc-600">{detail}</span> : null}
      </span>
      {badge ? (
        <span className={`inline-flex h-4 w-4 shrink-0 items-center justify-center rounded text-[10px] font-bold ${badge.className}`}>{badge.label}</span>
      ) : null}
    </button>
  );
}

/** The file's lines with a gutter. Two blocks of text rather than a row per line, so a large file stays cheap. */
function FileText({ content, error }: { content: FileContent | null; error: string | null }) {
  if (error) return <div className="p-6 text-sm text-red-300">{error}</div>;
  if (!content) return <div className="p-6 text-sm text-zinc-500">Reading file…</div>;
  if (content.binary) return <div className="p-6 text-sm text-zinc-400">Binary file, {formatSize(content.size)}. Open it in your editor to view it.</div>;
  if (content.content === "") return <div className="p-6 text-sm text-zinc-500">This file is empty.</div>;

  const text = content.content.replace(/\n$/, "");
  const lines = text.split("\n");
  return (
    <div>
      {content.truncated ? (
        <div className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-1.5 text-[11px] text-amber-200">
          Showing the first {formatSize(content.content.length)} of {formatSize(content.size)}.
        </div>
      ) : null}
      <div className="flex font-mono text-[12px] leading-5">
        <pre aria-hidden className="shrink-0 select-none border-r border-zinc-800 px-3 text-right text-zinc-600">
          {lines.map((_, index) => index + 1).join("\n")}
        </pre>
        <pre className="min-w-0 flex-1 px-3 text-zinc-300">{text}</pre>
      </div>
    </div>
  );
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
