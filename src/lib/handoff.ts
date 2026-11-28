/** Longest brief or reply shown on one run-log line. The full text stays in the run journal. */
const LINE_CHARS = 220;

/** Collapses whitespace and cuts long text, keeping its start, so it fits on one log line. */
export function excerpt(text: string, max = LINE_CHARS): string {
  const flat = text.replace(/\s+/g, " ").trim();
  return flat.length > max ? `${flat.slice(0, max - 1).trimEnd()}…` : flat;
}

/** The first line of a brief that is not a markdown heading, such as "## Delegated task". */
function briefHeadline(prompt: string): string {
  return prompt
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.length > 0 && !line.startsWith("#")) ?? "";
}

/**
 * One run-log line for a handoff: what the caller asked the target to do, and what came back.
 * The journal stores both texts for every `handoff` event; this shows them so the exchange can
 * be read from the run log without opening the target's terminal.
 */
export function describeHandoff(payload: Record<string, unknown>, targetNodeId: string | null): string {
  const from = typeof payload.from === "string" ? payload.from : "?";
  const to = targetNodeId ?? "?";
  const brief = typeof payload.prompt === "string" ? excerpt(briefHeadline(payload.prompt)) : "";
  const status = typeof payload.status === "string" ? payload.status : "finished";
  const summary = typeof payload.summary === "string" ? excerpt(payload.summary) : "";

  const asked = brief ? ` asked "${brief}"` : "";
  const replied = summary ? ` · ${to} replied (${status}): "${summary}"` : ` · ${to} ${status}`;
  return `${from} → ${to}${asked}${replied}`;
}
