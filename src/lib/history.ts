import type { RelationType, RunEvent } from "../types/orchestration";

/**
 * One entry of a run's conversation: a handoff between agents, with what was asked and what came
 * back, or an approval decision on a gated handoff.
 */
export type TranscriptItem =
  | {
      kind: "handoff";
      key: string;
      at: number;
      from: string;
      to: string;
      relation: RelationType;
      prompt: string;
      status: string;
      summary: string;
      diffStat: string;
      exitCode: number | null;
    }
  | { kind: "approval"; key: string; at: number; edgeId: string; approved: boolean };

const text = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * The conversation in a run's journal, in order. Status changes and tool-call bookkeeping are
 * left out, because the handoffs already say what happened.
 */
export function transcriptItems(events: RunEvent[]): TranscriptItem[] {
  const items: TranscriptItem[] = [];
  for (const event of events) {
    const key = `event-${event.id}`;
    if (event.kind === "handoff") {
      const payload = event.payload;
      items.push({
        kind: "handoff",
        key,
        at: event.createdMs,
        from: text(payload.from),
        to: text(payload.to),
        relation: text(payload.relation) as RelationType,
        prompt: text(payload.prompt),
        status: text(payload.status),
        summary: text(payload.summary),
        diffStat: text(payload.diffStat),
        exitCode: typeof payload.exitCode === "number" ? payload.exitCode : null,
      });
    } else if (event.kind === "approval") {
      items.push({
        kind: "approval",
        key,
        at: event.createdMs,
        edgeId: text(event.payload.edgeId),
        approved: event.payload.approved === true,
      });
    }
  }
  return items;
}
