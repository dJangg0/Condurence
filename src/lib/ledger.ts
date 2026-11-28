/**
 * The run's task ledger, built from the journal events the runtime emits: the plan the orchestrator
 * wrote, every attempt on every task, and each merge into the integration branch. It is a fold over
 * events, so the live view and a replay of the journal after a reload read the same way.
 */

export type LedgerPlanTask = {
  id: string;
  title: string;
  dependsOn: string[];
  /** pending, running, accepted or stalled. */
  state: string;
};

export type LedgerAttempt = {
  stream: string;
  taskId: string | null;
  label: string;
  from: string;
  to: string;
  /** open, running, accepted, stalled or refused. */
  state: string;
  attempt: number | null;
  maxRetries: number;
  remaining: number | null;
  /** Why the latest call was refused, when it was. */
  reason: string | null;
  signature: string | null;
  /** The latest merge into the integration branch for this task: merged, conflict, error, and so on. */
  merge: string | null;
};

export type Ledger = {
  plan: LedgerPlanTask[];
  attempts: LedgerAttempt[];
};

export const EMPTY_LEDGER: Ledger = { plan: [], attempts: [] };

const PLAN_STATE: Record<string, string> = {
  running: "running",
  accepted: "accepted",
  stalled: "stalled",
  open: "pending",
  pending: "pending",
};

function text(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function count(value: unknown): number | null {
  return typeof value === "number" ? value : null;
}

/** Folds one journal event into the ledger. Events the ledger does not show leave it unchanged. */
export function applyLedgerEvent(
  ledger: Ledger,
  kind: string,
  payload: Record<string, unknown>,
  _nodeId: string | null,
): Ledger {
  switch (kind) {
    case "plan": {
      const tasks = Array.isArray(payload.tasks) ? (payload.tasks as Record<string, unknown>[]) : [];
      return {
        ...ledger,
        plan: tasks.map((task) => ({
          id: text(task.id) ?? "",
          title: text(task.title) ?? "",
          dependsOn: Array.isArray(task.dependsOn) ? task.dependsOn.filter((dep): dep is string => typeof dep === "string") : [],
          state: text(task.state) ?? "pending",
        })),
      };
    }
    case "task": {
      const stream = text(payload.stream);
      if (!stream) return ledger;
      const taskId = text(payload.taskId);
      const state = text(payload.state) ?? "open";
      const previous = ledger.attempts.find((attempt) => attempt.stream === stream);
      const next: LedgerAttempt = {
        stream,
        taskId,
        label: text(payload.task) ?? previous?.label ?? stream,
        from: text(payload.from) ?? previous?.from ?? "",
        to: text(payload.to) ?? previous?.to ?? "",
        state,
        attempt: count(payload.attempt) ?? previous?.attempt ?? null,
        maxRetries: count(payload.maxRetries) ?? previous?.maxRetries ?? 0,
        remaining: count(payload.remaining) ?? previous?.remaining ?? null,
        reason: state === "refused" ? text(payload.reason) : null,
        signature: text(payload.signature) ?? (state === "running" ? null : previous?.signature ?? null),
        merge: previous?.merge ?? null,
      };
      const attempts = previous
        ? ledger.attempts.map((attempt) => (attempt.stream === stream ? next : attempt))
        : [...ledger.attempts, next];
      const plan =
        taskId && state !== "refused"
          ? ledger.plan.map((task) => (task.id === taskId ? { ...task, state: PLAN_STATE[state] ?? task.state } : task))
          : ledger.plan;
      return { plan, attempts };
    }
    case "merge": {
      const label = text(payload.task);
      if (!label) return ledger;
      // The merge belongs to the newest attempt with the same label, which is the one that just ran.
      const index = [...ledger.attempts].reverse().findIndex((attempt) => attempt.label === label);
      if (index < 0) return ledger;
      const at = ledger.attempts.length - 1 - index;
      const attempts = ledger.attempts.map((attempt, position) =>
        position === at ? { ...attempt, merge: text(payload.state) } : attempt,
      );
      return { ...ledger, attempts };
    }
    default:
      return ledger;
  }
}

/** The ledger for a whole journal, in order. Used to show a run that is already finished. */
export function ledgerFromEvents(events: { kind: string; payload: Record<string, unknown>; nodeId: string | null }[]): Ledger {
  return events.reduce((ledger, event) => applyLedgerEvent(ledger, event.kind, event.payload, event.nodeId), EMPTY_LEDGER);
}
