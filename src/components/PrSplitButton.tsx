import { useEffect, useRef, useState } from "react";
import { ChevronDownIcon, PullRequestIcon } from "./icons";
import type { PrControl, PrItem, PrItemId } from "../lib/changes";

const BUTTON = "inline-flex items-center gap-1.5 px-2.5 py-1 text-xs font-medium disabled:cursor-default disabled:opacity-50";

/**
 * Create PR as a split button, the way Conductor lays it out: the main action on the left, and a
 * chevron on the right that opens the other choices. The button only reports which item was
 * pressed. It never opens, commits or pushes anything itself.
 */
export function PrSplitButton({
  control,
  busy,
  busyLabel,
  onSelect,
}: {
  control: PrControl;
  /** An action is running, so the buttons hold still. */
  busy: boolean;
  /** Replaces the main label while that action runs, such as "Creating PR…". */
  busyLabel?: string;
  onSelect: (id: PrItemId) => void;
}) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  // The menu closes on a click elsewhere and on Escape. Escape is stopped here so it does not
  // also close the panel the button sits in.
  useEffect(() => {
    if (!open) return;
    const onPointer = (event: MouseEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setOpen(false);
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [open]);

  const { primary, menu } = control;
  const label = busy && busyLabel ? busyLabel : primary.label;
  const pick = (item: PrItem) => {
    setOpen(false);
    onSelect(item.id);
  };
  const prominent = primary.id === "create" || primary.id === "commit-and-create";

  return (
    <div ref={root} className="relative inline-flex rounded-md border border-zinc-600 bg-zinc-800/80 text-zinc-100">
      {primary.href ? (
        <a href={primary.href} target="_blank" rel="noreferrer" className={`${BUTTON} rounded-l-md hover:bg-zinc-700`}>
          <PullRequestIcon />
          {label}
        </a>
      ) : (
        <button
          type="button"
          onClick={() => onSelect(primary.id)}
          disabled={busy || primary.disabled}
          title={primary.disabled ? "Reading this branch's PR status…" : undefined}
          className={`${BUTTON} rounded-l-md hover:bg-zinc-700 ${prominent ? "" : "text-zinc-300"}`}
        >
          <PullRequestIcon />
          {label}
        </button>
      )}
      <button
        type="button"
        aria-label="More pull request actions"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        disabled={busy}
        className="inline-flex items-center rounded-r-md border-l border-zinc-600 px-1.5 hover:bg-zinc-700 disabled:opacity-50"
      >
        <ChevronDownIcon />
      </button>
      {open ? (
        <div role="menu" className="absolute right-0 top-full z-50 mt-1 min-w-64 whitespace-nowrap rounded-md border border-zinc-700 bg-zinc-900 p-1 shadow-xl">
          {menu.map((item) =>
            item.href ? (
              <a
                key={item.id}
                role="menuitem"
                href={item.href}
                target="_blank"
                rel="noreferrer"
                onClick={() => setOpen(false)}
                className="block rounded px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-zinc-800"
              >
                {item.label}
              </a>
            ) : (
              <button
                key={item.id}
                type="button"
                role="menuitem"
                onClick={() => pick(item)}
                className="block w-full rounded px-2.5 py-1.5 text-left text-xs text-zinc-200 hover:bg-zinc-800"
              >
                {item.label}
              </button>
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}
