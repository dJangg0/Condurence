import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { GitSettingsPanel } from "./RepoSettingsView";
import { DEFAULT_REPO_SETTINGS } from "../lib/repoSettings";
import type { RepoGitSettings } from "../types/orchestration";

const panel = (settings: RepoGitSettings = DEFAULT_REPO_SETTINGS, extra: Partial<Parameters<typeof GitSettingsPanel>[0]> = {}) =>
  renderToStaticMarkup(
    <GitSettingsPanel
      settings={settings}
      remotes={["origin", "upstream"]}
      branches={["origin/main", "origin/beta"]}
      onChange={() => undefined}
      onRemoteChange={() => undefined}
      error={null}
      saving={false}
      {...extra}
    />,
  );

describe("GitSettingsPanel", () => {
  it("shows the Git section with the five controls from Conductor's page", () => {
    const markup = panel();
    expect(markup).toContain(">Git</h2>");
    expect(markup).toContain("Branch new workspaces from");
    expect(markup).toContain("Each workspace is an isolated copy of your codebase.");
    expect(markup).toContain("Remote origin");
    expect(markup).toContain("Where should we push, pull, and create PRs?");
    expect(markup).toContain("Delete branch on archive");
    expect(markup).toContain("Archive on merge");
    expect(markup).toContain("Set upstream on plain git push");
  });

  it("reflects the saved toggles", () => {
    const markup = panel({ ...DEFAULT_REPO_SETTINGS, archiveOnMerge: true, setUpstreamOnPush: false });
    expect(markup).toMatch(/aria-checked="true"[^>]*aria-label="Archive on merge"/);
    expect(markup).toMatch(/aria-checked="false"[^>]*aria-label="Delete branch on archive"/);
    expect(markup).toMatch(/aria-checked="false"[^>]*aria-label="Set upstream on plain git push"/);
  });

  it("offers the remote's branches as the base, keeping the saved base even if it is not listed", () => {
    const markup = panel({ ...DEFAULT_REPO_SETTINGS, baseRef: "origin/staging" });
    expect(markup).toContain(">origin/staging</option>");
    expect(markup).toContain(">origin/main</option>");
    expect(markup).toContain(">origin/beta</option>");
  });

  it("says when a change is being saved or has failed", () => {
    expect(panel(DEFAULT_REPO_SETTINGS, { saving: true })).toContain("Saving…");
    expect(panel(DEFAULT_REPO_SETTINGS, { error: "remote is gone" })).toContain("remote is gone");
  });
});
