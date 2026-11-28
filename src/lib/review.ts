import type { TranscriptItem } from "./history";

export type ReviewAgent = "claude" | "codex";

/** What the review drawer offers. It is remembered per repository, so a review is one click. */
export type ReviewOptions = {
  committed: boolean;
  uncommitted: boolean;
  commitLog: boolean;
  includeGoal: boolean;
  includeHandoffs: boolean;
  testCommand: string;
  focus: string;
  agent: ReviewAgent;
};

export const DEFAULT_REVIEW_OPTIONS: ReviewOptions = {
  committed: true,
  uncommitted: true,
  commitLog: false,
  includeGoal: true,
  includeHandoffs: true,
  testCommand: "",
  focus: "",
  agent: "claude",
};

/** The context the backend turns into the review prompt. Matches `ReviewContext` in `pulls`. */
export type ReviewContextPayload = {
  committed: boolean;
  uncommitted: boolean;
  commitLog: boolean;
  goal: string | null;
  handoffs: string | null;
  testCommand: string | null;
  focus: string | null;
};

export function canStartReview(options: ReviewOptions): boolean {
  return options.committed || options.uncommitted;
}

/** The payload for the chosen options. Ticked-off context and blank text are left out. */
export function reviewContextFor(options: ReviewOptions, goal: string, handoffs: string): ReviewContextPayload {
  return {
    committed: options.committed,
    uncommitted: options.uncommitted,
    commitLog: options.commitLog,
    goal: options.includeGoal && goal.trim() ? goal.trim() : null,
    handoffs: options.includeHandoffs && handoffs.trim() ? handoffs : null,
    testCommand: options.testCommand.trim() || null,
    focus: options.focus.trim() || null,
  };
}

/** The run's handoffs, one line each, as the reviewer reads them. */
export function handoffSummary(items: TranscriptItem[], nameOf: (nodeId: string) => string): string {
  return items
    .map((item) =>
      item.kind === "handoff"
        ? `- ${nameOf(item.from)} → ${nameOf(item.to)} (${item.status || "pending"}): ${item.summary || "no summary"}`
        : `- ${item.approved ? "Approved" : "Rejected"} a gated handoff`,
    )
    .join("\n");
}

type KeyValueStorage = { getItem(key: string): string | null; setItem(key: string, value: string): void };

const keyFor = (repoPath: string) => `condurence.review.${repoPath}`;

/** Stored options for a repository, or the defaults. A damaged record falls back to the defaults. */
export function loadReviewOptions(repoPath: string, storage: KeyValueStorage | undefined = browserStorage()): ReviewOptions {
  try {
    const stored = storage?.getItem(keyFor(repoPath));
    if (!stored) return { ...DEFAULT_REVIEW_OPTIONS };
    const parsed = JSON.parse(stored) as Partial<ReviewOptions>;
    return {
      ...DEFAULT_REVIEW_OPTIONS,
      ...parsed,
      agent: parsed.agent === "codex" ? "codex" : "claude",
    };
  } catch {
    return { ...DEFAULT_REVIEW_OPTIONS };
  }
}

export function saveReviewOptions(
  repoPath: string,
  options: ReviewOptions,
  storage: KeyValueStorage | undefined = browserStorage(),
): void {
  try {
    storage?.setItem(keyFor(repoPath), JSON.stringify(options));
  } catch {
    // A full or blocked store only costs the remembered options, never the review.
  }
}

function browserStorage(): KeyValueStorage | undefined {
  return typeof localStorage === "undefined" ? undefined : localStorage;
}
