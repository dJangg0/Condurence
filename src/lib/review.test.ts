import { describe, expect, it } from "vitest";
import {
  DEFAULT_REVIEW_OPTIONS,
  canStartReview,
  handoffSummary,
  loadReviewOptions,
  reviewContextFor,
  saveReviewOptions,
} from "./review";
import type { TranscriptItem } from "./history";

/** An in-memory stand-in for localStorage. */
const memory = () => {
  const data = new Map<string, string>();
  return {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
  };
};

const handoff: TranscriptItem = {
  kind: "handoff",
  key: "event-1",
  at: 1,
  from: "ceo",
  to: "senior-dev",
  relation: "delegates_to",
  prompt: "add the toggle",
  status: "completed",
  summary: "added it",
  diffStat: "",
  exitCode: 0,
};

describe("review options", () => {
  it("review both committed and uncommitted work by default, with the goal and handoffs", () => {
    expect(DEFAULT_REVIEW_OPTIONS).toMatchObject({ committed: true, uncommitted: true, includeGoal: true, agent: "claude" });
    expect(canStartReview(DEFAULT_REVIEW_OPTIONS)).toBe(true);
    expect(canStartReview({ ...DEFAULT_REVIEW_OPTIONS, committed: false, uncommitted: false })).toBe(false);
  });

  it("are remembered per repository and fall back to the defaults", () => {
    const storage = memory();
    saveReviewOptions("/repo-a", { ...DEFAULT_REVIEW_OPTIONS, focus: "races", agent: "codex" }, storage);
    expect(loadReviewOptions("/repo-a", storage)).toMatchObject({ focus: "races", agent: "codex" });
    expect(loadReviewOptions("/repo-b", storage)).toEqual(DEFAULT_REVIEW_OPTIONS);
  });

  it("survive a damaged record, an unknown agent, and no storage at all", () => {
    const storage = memory();
    storage.setItem("condurence.review./repo", "not json");
    expect(loadReviewOptions("/repo", storage)).toEqual(DEFAULT_REVIEW_OPTIONS);
    storage.setItem("condurence.review./repo", JSON.stringify({ agent: "gemini" }));
    expect(loadReviewOptions("/repo", storage).agent).toBe("claude");
    expect(loadReviewOptions("/repo", undefined)).toEqual(DEFAULT_REVIEW_OPTIONS);
    expect(() => saveReviewOptions("/repo", DEFAULT_REVIEW_OPTIONS, undefined)).not.toThrow();
  });
});

describe("reviewContextFor", () => {
  it("sends only the context that is ticked, with blank text left out", () => {
    const payload = reviewContextFor(
      { ...DEFAULT_REVIEW_OPTIONS, includeHandoffs: false, testCommand: "  npm test  ", focus: "   " },
      "add dark mode",
      "- ceo → senior-dev",
    );
    expect(payload).toEqual({
      committed: true,
      uncommitted: true,
      commitLog: false,
      goal: "add dark mode",
      handoffs: null,
      testCommand: "npm test",
      focus: null,
    });
  });

  it("drops the goal when it is not wanted or empty", () => {
    expect(reviewContextFor({ ...DEFAULT_REVIEW_OPTIONS, includeGoal: false }, "goal", "").goal).toBeNull();
    expect(reviewContextFor(DEFAULT_REVIEW_OPTIONS, "   ", "").goal).toBeNull();
  });
});

describe("handoffSummary", () => {
  it("lists each handoff by agent name with its result, and approvals", () => {
    const names: Record<string, string> = { ceo: "CEO", "senior-dev": "Senior Dev" };
    const text = handoffSummary(
      [handoff, { kind: "approval", key: "event-2", at: 2, edgeId: "e1", approved: false }],
      (id) => names[id] ?? id,
    );
    expect(text).toBe("- CEO → Senior Dev (completed): added it\n- Rejected a gated handoff");
  });
});
