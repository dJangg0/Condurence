import { describe, expect, it } from "vitest";
import { EMPTY_LEDGER, applyLedgerEvent, ledgerFromEvents } from "./ledger";

const planned = {
  tasks: [
    { id: "a", title: "Add the schema", dependsOn: [], state: "pending" },
    { id: "b", title: "Expose it", dependsOn: ["a"], state: "pending" },
  ],
};

describe("applyLedgerEvent", () => {
  it("records the plan with its dependencies", () => {
    const ledger = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    expect(ledger.plan).toEqual([
      { id: "a", title: "Add the schema", dependsOn: [], state: "pending" },
      { id: "b", title: "Expose it", dependsOn: ["a"], state: "pending" },
    ]);
  });

  it("moves an attempt and its planned task through running to accepted", () => {
    let ledger = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    ledger = applyLedgerEvent(
      ledger,
      "task",
      { stream: "e1#a", taskId: "a", task: "a", from: "ceo", to: "senior-dev", maxRetries: 3, state: "running", attempt: 1, remaining: 2 },
      "senior-dev",
    );
    expect(ledger.attempts).toHaveLength(1);
    expect(ledger.attempts[0]).toMatchObject({ state: "running", attempt: 1, remaining: 2, to: "senior-dev" });
    expect(ledger.plan[0].state).toBe("running");

    ledger = applyLedgerEvent(
      ledger,
      "task",
      { stream: "e1#a", taskId: "a", task: "a", from: "ceo", to: "senior-dev", maxRetries: 3, state: "accepted", attempt: 1, remaining: 2 },
      "senior-dev",
    );
    expect(ledger.attempts).toHaveLength(1);
    expect(ledger.attempts[0].state).toBe("accepted");
    expect(ledger.plan[0].state).toBe("accepted");
    expect(ledger.plan[1].state).toBe("pending");
  });

  it("keeps a refused call's reason and leaves the plan alone", () => {
    let ledger = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    ledger = applyLedgerEvent(
      ledger,
      "task",
      { stream: "e1#b", taskId: "b", task: "b", from: "ceo", to: "senior-dev", maxRetries: 3, state: "refused", reason: "task 'b' waits for a" },
      "senior-dev",
    );
    expect(ledger.attempts[0]).toMatchObject({ state: "refused", reason: "task 'b' waits for a" });
    expect(ledger.plan[1].state).toBe("pending");
  });

  it("marks a stalled task in the plan and clears the reason of later calls", () => {
    let ledger = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    ledger = applyLedgerEvent(
      ledger,
      "task",
      { stream: "e1#a", taskId: "a", task: "a", state: "refused", reason: "earlier" },
      null,
    );
    ledger = applyLedgerEvent(
      ledger,
      "task",
      { stream: "e1#a", taskId: "a", task: "a", state: "stalled", attempt: 2, remaining: 1, signature: "gate:test:exit1:x" },
      null,
    );
    expect(ledger.plan[0].state).toBe("stalled");
    expect(ledger.attempts[0]).toMatchObject({ state: "stalled", reason: null, signature: "gate:test:exit1:x" });
  });

  it("attaches a merge to the newest attempt with the same label", () => {
    let ledger = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    ledger = applyLedgerEvent(ledger, "task", { stream: "s1", task: "delegates to senior-dev", state: "running", attempt: 1 }, null);
    ledger = applyLedgerEvent(ledger, "task", { stream: "s1", task: "delegates to senior-dev", state: "open", attempt: 1 }, null);
    ledger = applyLedgerEvent(ledger, "task", { stream: "s2", task: "delegates to senior-dev", state: "running", attempt: 2 }, null);
    ledger = applyLedgerEvent(ledger, "merge", { task: "delegates to senior-dev", state: "conflict" }, "senior-dev");
    expect(ledger.attempts[0].merge).toBeNull();
    expect(ledger.attempts[1].merge).toBe("conflict");
  });

  it("ignores merges that belong to no attempt and events it does not show", () => {
    const before = applyLedgerEvent(EMPTY_LEDGER, "plan", planned, null);
    expect(applyLedgerEvent(before, "merge", { task: "nothing", state: "merged" }, null)).toBe(before);
    expect(applyLedgerEvent(before, "gate", { gate: "test", passed: true }, null)).toBe(before);
    expect(applyLedgerEvent(before, "status", { status: "running" }, null)).toBe(before);
  });
});

describe("ledgerFromEvents", () => {
  it("replays a finished run the same way the live view built it", () => {
    const events = [
      { kind: "plan", payload: planned, nodeId: null },
      { kind: "task", payload: { stream: "e1#a", taskId: "a", task: "a", state: "running", attempt: 1 }, nodeId: "senior-dev" },
      { kind: "merge", payload: { task: "a", state: "merged" }, nodeId: "senior-dev" },
      { kind: "task", payload: { stream: "e1#a", taskId: "a", task: "a", state: "accepted", attempt: 1 }, nodeId: "senior-dev" },
    ];
    const ledger = ledgerFromEvents(events);
    expect(ledger.plan[0].state).toBe("accepted");
    expect(ledger.attempts[0]).toMatchObject({ state: "accepted", merge: "merged" });
  });
});
