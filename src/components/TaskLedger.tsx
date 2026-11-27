import type { Ledger } from "../lib/ledger";

const STATE_TONE: Record<string, string> = {
  pending: "text-zinc-400",
  open: "text-zinc-300",
  running: "text-indigo-300",
  accepted: "text-emerald-300",
  stalled: "text-amber-300",
  refused: "text-red-300",
};

const MERGE_TONE: Record<string, string> = {
  merged: "text-emerald-300",
  "nothing-to-merge": "text-zinc-400",
  conflict: "text-amber-300",
  error: "text-red-300",
};

/**
 * The run's tasks: the plan the orchestrator wrote, and each attempt on each task with its budget,
 * its state, why a call was refused, the failure it repeated, and how its work merged.
 */
export function TaskLedger({ ledger, nameOf }: { ledger: Ledger; nameOf: (nodeId: string) => string }) {
  if (ledger.plan.length === 0 && ledger.attempts.length === 0) {
    return <div className="p-3 text-xs text-zinc-500">Tasks appear here once the orchestrator plans or delegates work.</div>;
  }
  return (
    <div className="space-y-4 p-3 text-xs">
      {ledger.plan.length > 0 ? (
        <section>
          <div className="mb-1.5 text-[11px] uppercase tracking-wider text-zinc-500">Plan</div>
          <ol className="space-y-1.5">
            {ledger.plan.map((task) => (
              <li key={task.id} className="flex items-start gap-2">
                <span className={`w-16 shrink-0 font-mono ${STATE_TONE[task.state] ?? "text-zinc-400"}`}>{task.state}</span>
                <span className="min-w-0 text-zinc-200">
                  <span className="font-mono text-indigo-300">{task.id}</span> {task.title}
                  {task.dependsOn.length > 0 ? <span className="text-zinc-500"> · after {task.dependsOn.join(", ")}</span> : null}
                </span>
              </li>
            ))}
          </ol>
        </section>
      ) : null}

      {ledger.attempts.length > 0 ? (
        <section>
          <div className="mb-1.5 text-[11px] uppercase tracking-wider text-zinc-500">Attempts</div>
          <ul className="space-y-2">
            {ledger.attempts.map((attempt) => (
              <li key={attempt.stream} className="rounded-md border border-zinc-800 bg-zinc-950/60 p-2">
                <div className="flex items-center gap-2">
                  <span className={`font-mono ${STATE_TONE[attempt.state] ?? "text-zinc-400"}`}>{attempt.state}</span>
                  <span className="min-w-0 truncate text-zinc-200">{attempt.label}</span>
                  <span className="ml-auto shrink-0 font-mono text-zinc-500">
                    {attempt.attempt ?? "–"} of {attempt.maxRetries}
                  </span>
                </div>
                <div className="mt-0.5 text-zinc-500">
                  {attempt.from ? `${nameOf(attempt.from)} → ` : ""}
                  {attempt.to ? nameOf(attempt.to) : ""}
                  {attempt.merge ? (
                    <span className={`ml-2 ${MERGE_TONE[attempt.merge] ?? "text-zinc-400"}`}>· merge {attempt.merge}</span>
                  ) : null}
                </div>
                {attempt.reason ? <div className="mt-1 text-red-300">{attempt.reason}</div> : null}
                {attempt.signature && attempt.state !== "accepted" ? (
                  <div className="mt-1 break-all font-mono text-[11px] text-amber-200/80">{attempt.signature}</div>
                ) : null}
              </li>
            ))}
          </ul>
        </section>
      ) : null}
    </div>
  );
}
