type IconProps = { className?: string };

const base = {
  width: 14,
  height: 14,
  viewBox: "0 0 16 16",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
};

/** Branch-and-leaf glyph for isolated workspaces. */
export function WorkspacesIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <circle cx="4" cy="3.5" r="1.5" />
      <circle cx="4" cy="12.5" r="1.5" />
      <circle cx="12" cy="6" r="1.5" />
      <path d="M4 5v6" />
      <path d="M12 7.5c0 3-8 1.5-8 3.5" />
    </svg>
  );
}

/** Root node fanning out to two children, for agent orchestration. */
export function OrchestrationIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <rect x="6" y="1.5" width="4" height="3" rx="1" />
      <rect x="1.5" y="11.5" width="4" height="3" rx="1" />
      <rect x="10.5" y="11.5" width="4" height="3" rx="1" />
      <path d="M8 4.5v3M3.5 11.5V7.5h9v4" />
    </svg>
  );
}

export function ChevronUpDownIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M5 6l3-3 3 3M5 10l3 3 3-3" />
    </svg>
  );
}

export function CheckIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M3.5 8.5l3 3 6-6.5" />
    </svg>
  );
}

export function ArchiveIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <rect x="1.5" y="2.5" width="13" height="3.5" rx="1" />
      <path d="M2.5 6v6.5a1 1 0 0 0 1 1h9a1 1 0 0 0 1-1V6M6.5 9h3" />
    </svg>
  );
}

export function RestoreIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M2.5 8a5.5 5.5 0 1 0 1.8-4.1M2.5 2.5v3h3" />
    </svg>
  );
}

export function TrashIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M2.5 4h11M6 4V2.5h4V4M4 4l.7 9a1 1 0 0 0 1 .9h4.6a1 1 0 0 0 1-.9L12 4M6.5 7v4M9.5 7v4" />
    </svg>
  );
}

export function PlusIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M8 3v10M3 8h10" />
    </svg>
  );
}

export function ChevronDownIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M4 6l4 4 4-4" />
    </svg>
  );
}

export function ChevronRightIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M6 4l4 4-4 4" />
    </svg>
  );
}

export function GearIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <circle cx="8" cy="8" r="2" />
      <path d="M8 1.5v2M8 12.5v2M1.5 8h2M12.5 8h2M3.4 3.4l1.4 1.4M11.2 11.2l1.4 1.4M3.4 12.6l1.4-1.4M11.2 4.8l1.4-1.4" />
    </svg>
  );
}

export function PlayIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className} fill="currentColor" stroke="none">
      <path d="M4.5 2.8v10.4a.5.5 0 0 0 .77.42l8-5.2a.5.5 0 0 0 0-.84l-8-5.2a.5.5 0 0 0-.77.42Z" />
    </svg>
  );
}

export function RefreshIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9M13.5 2.5v3h-3" />
    </svg>
  );
}

export function TerminalIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M3 4.5l3.5 3.5L3 11.5M8 12h5" />
    </svg>
  );
}

/** Two nodes joined by a curve, the pull request glyph. */
export function PullRequestIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <circle cx="4.5" cy="3.5" r="1.5" />
      <circle cx="4.5" cy="12.5" r="1.5" />
      <circle cx="11.5" cy="12.5" r="1.5" />
      <path d="M4.5 5v6M11.5 11V7a2 2 0 0 0-2-2H8m0 0l1.5-1.5M8 5l1.5 1.5" />
    </svg>
  );
}

export function FolderIcon({ className, open }: IconProps & { open?: boolean }) {
  return (
    <svg {...base} className={className}>
      {open ? (
        <path d="M2 5.5V4a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V6M2 5.5h12l-1.2 6.2a1 1 0 0 1-1 .8H3.2a1 1 0 0 1-1-.8L2 5.5z" />
      ) : (
        <path d="M2 4a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4z" />
      )}
    </svg>
  );
}

export function FileIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <path d="M4 2.5h5l3 3V13a.5.5 0 0 1-.5.5h-7A.5.5 0 0 1 4 13V2.5zM9 2.5V6h3" />
    </svg>
  );
}

export function SearchIcon({ className }: IconProps) {
  return (
    <svg {...base} className={className}>
      <circle cx="7" cy="7" r="4" />
      <path d="M10 10l3.5 3.5" />
    </svg>
  );
}
