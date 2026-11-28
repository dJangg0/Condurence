/** What Enter in the goal field does. */
export type GoalSubmit = { kind: "none" } | { kind: "start"; goal: string } | { kind: "send"; message: string };

/**
 * Enter in the goal field. With no run it starts one from the typed goal. While a run is live the
 * same field talks to the CEO, so a task never needs the Run button again. Nothing happens while a
 * request is in flight, or when the field is empty or there are no agents to run.
 */
export function goalSubmit(input: { text: string; runActive: boolean; busy: boolean; hasAgents: boolean }): GoalSubmit {
  const text = input.text.trim();
  if (input.busy || !text) return { kind: "none" };
  if (input.runActive) return { kind: "send", message: text };
  if (!input.hasAgents) return { kind: "none" };
  return { kind: "start", goal: text };
}
