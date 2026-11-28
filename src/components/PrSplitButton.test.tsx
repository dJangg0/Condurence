import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { PrSplitButton } from "./PrSplitButton";
import { prControl } from "../lib/changes";
import type { PullRequestStatus } from "../types/pulls";

const pr = (state: string): PullRequestStatus => ({
  number: 7,
  url: "https://github.com/acme/demo/pull/7",
  state,
  isDraft: false,
  mergeable: "MERGEABLE",
  mergeStateStatus: "CLEAN",
  reviewDecision: null,
  checks: "passing",
});

const input = { branch: "aatrox", base: "main", changedFiles: 0, pr: null };
const render = (overrides: Partial<Parameters<typeof prControl>[0]> = {}, props: { busy?: boolean; busyLabel?: string } = {}) => {
  const control = prControl({ ...input, ...overrides });
  if (!control) throw new Error("no control");
  return renderToStaticMarkup(<PrSplitButton control={control} busy={props.busy ?? false} busyLabel={props.busyLabel} onSelect={() => undefined} />);
};

describe("PrSplitButton", () => {
  it("shows Create PR with a chevron for the other actions, and keeps the menu closed", () => {
    const markup = render();
    expect(markup).toContain("Create PR");
    expect(markup).toContain('aria-label="More pull request actions"');
    expect(markup).toContain('aria-expanded="false"');
    expect(markup).not.toContain('role="menu"');
  });

  it("asks to commit first when the workspace has uncommitted edits", () => {
    expect(render({ changedFiles: 2 })).toContain("Commit &amp; create PR…");
  });

  it("links to the PR once it exists", () => {
    const markup = render({ pr: pr("OPEN") });
    expect(markup).toContain('href="https://github.com/acme/demo/pull/7"');
    expect(markup).toContain("PR #7 · open");
    expect(markup).not.toContain("Create PR");
  });

  it("is disabled while the PR status is unknown, with the reason as its title", () => {
    expect(render({ pr: undefined })).toMatch(/disabled=""[^>]*title="Reading this branch/);
  });

  it("holds still and names the running action while it works", () => {
    const markup = render({}, { busy: true, busyLabel: "Creating PR…" });
    expect(markup).toContain("Creating PR…");
    expect(markup.match(/disabled=""/g)).toHaveLength(2);
  });
});
