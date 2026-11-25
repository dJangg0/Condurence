import type { FileDiff, FileStatus } from "../types/orchestration";

const STATUS_BADGE: Record<FileStatus, { label: string; className: string }> = {
  added: { label: "A", className: "text-emerald-300 bg-emerald-500/15" },
  untracked: { label: "U", className: "text-emerald-300 bg-emerald-500/15" },
  modified: { label: "M", className: "text-amber-300 bg-amber-500/15" },
  deleted: { label: "D", className: "text-red-300 bg-red-500/15" },
  renamed: { label: "R", className: "text-sky-300 bg-sky-500/15" },
  conflicted: { label: "!", className: "text-red-300 bg-red-500/25" },
};

/** Changed-file list on the left, side-by-side-ready hunks on the right. */
export function FileList({
  files,
  selected,
  onSelect,
}: {
  files: FileDiff[];
  selected: string | null;
  onSelect: (path: string) => void;
}) {
  if (files.length === 0) {
    return <div className="p-4 text-sm text-zinc-500">No changes in this workspace.</div>;
  }
  return (
    <ul className="space-y-0.5 p-1.5">
      {files.map((file) => {
        const badge = STATUS_BADGE[file.status];
        const active = file.path === selected;
        return (
          <li key={file.path}>
            <button
              type="button"
              onClick={() => onSelect(file.path)}
              className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs ${
                active ? "bg-zinc-800 text-zinc-100" : "text-zinc-300 hover:bg-zinc-800/60"
              }`}
            >
              <span className={`inline-flex h-4 w-4 shrink-0 items-center justify-center rounded text-[10px] font-bold ${badge.className}`}>
                {badge.label}
              </span>
              <span className="min-w-0 flex-1 truncate font-mono" title={file.path}>
                {file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
              </span>
              <span className="shrink-0 font-mono text-[10px]">
                <span className="text-emerald-400">+{file.additions}</span>{" "}
                <span className="text-red-400">-{file.deletions}</span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}

export function DiffDetail({ file }: { file: FileDiff | undefined }) {
  if (!file) {
    return <div className="p-6 text-sm text-zinc-500">Select a file to review its changes.</div>;
  }
  if (file.binary) {
    return <div className="p-6 text-sm text-zinc-400">Binary or oversized file. Open it in your editor to review.</div>;
  }
  if (file.hunks.length === 0) {
    return <div className="p-6 text-sm text-zinc-400">No textual changes.</div>;
  }
  return (
    <div className="font-mono text-[12px] leading-5">
      {file.hunks.map((hunk, hunkIndex) => (
        <div key={`${hunk.header}-${hunkIndex}`}>
          <div className="sticky top-0 bg-zinc-900/95 px-3 py-1 text-[11px] text-indigo-300">{hunk.header}</div>
          {hunk.lines.map((line, lineIndex) => {
            const tone =
              line.kind === "add" ? "bg-emerald-500/10 text-emerald-200" : line.kind === "del" ? "bg-red-500/10 text-red-200" : "text-zinc-400";
            const marker = line.kind === "add" ? "+" : line.kind === "del" ? "-" : " ";
            return (
              <div key={lineIndex} className={`flex ${tone}`}>
                <span className="w-10 shrink-0 select-none border-r border-zinc-800 px-1 text-right text-zinc-600">
                  {line.oldLine ?? ""}
                </span>
                <span className="w-10 shrink-0 select-none border-r border-zinc-800 px-1 text-right text-zinc-600">
                  {line.newLine ?? ""}
                </span>
                <span className="w-4 shrink-0 select-none text-center">{marker}</span>
                <span className="whitespace-pre pr-3">{line.content}</span>
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
