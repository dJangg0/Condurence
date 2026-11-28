import type { RunChangeCount } from "../lib/changes";

/** `1234` as `1,234`, whatever the machine's locale, so the diffstat reads the same everywhere. */
const group = (value: number) => value.toLocaleString("en-US");

/**
 * Opens the review pane. Once the run has changed something, the button is lit and carries the
 * diffstat the way GitHub shows it, lines added in green and lines removed in red, so it is plain
 * from the toolbar how big the change is.
 */
export function ChangesButton({
  count,
  hasRun,
  onClick,
}: {
  count: RunChangeCount | null;
  /** A run exists to review. */
  hasRun: boolean;
  onClick: () => void;
}) {
  const lit = count !== null && count.files > 0;
  const noLines = count !== null && count.additions + count.deletions === 0;
  const fileWord = count?.files === 1 ? "file" : "files";
  const title = !hasRun
    ? "Run the topology first, then review what the agents changed"
    : lit && count
      ? `${group(count.additions)} lines added, ${group(count.deletions)} removed in ${count.files} ${fileWord}${
          count.workspaces > 1 ? ` across ${count.workspaces} workspaces` : ""
        }. Review them, browse the files and open a PR`
      : "No changes yet. Review the run, browse its files and open a PR";
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={!hasRun}
      title={title}
      className={`inline-flex items-center gap-2 rounded-md border px-3 py-1.5 text-xs font-medium disabled:cursor-not-allowed disabled:opacity-40 ${
        lit ? "border-zinc-500 bg-zinc-800 text-zinc-100 hover:bg-zinc-700" : "border-zinc-700 text-zinc-200 hover:bg-zinc-800"
      }`}
    >
      Changes
      {lit && count ? (
        noLines ? (
          // A change with no text lines, such as a binary file: the files are the only number to show.
          <span className="font-mono text-[11px] text-zinc-300">
            {count.files} {fileWord}
          </span>
        ) : (
          <span className="inline-flex gap-1.5 font-mono text-[11px] font-semibold" aria-label={`${count.additions} added, ${count.deletions} removed`}>
            <span className="text-emerald-400">+{group(count.additions)}</span>
            <span className="text-red-400">−{group(count.deletions)}</span>
          </span>
        )
      ) : null}
    </button>
  );
}
