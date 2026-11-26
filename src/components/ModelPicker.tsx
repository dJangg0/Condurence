import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import type { AgentProvider, ModelOption } from "../types/orchestration";

const FIELD =
  "w-full rounded-md border border-zinc-700 bg-zinc-950 px-2.5 py-1.5 text-sm text-zinc-100 outline-none focus:border-indigo-500";

const DEFAULT_VALUE = "";
const CUSTOM_VALUE = "__custom__";

// Model lists change rarely, so each provider is fetched once per session.
const cache = new Map<AgentProvider, Promise<ModelOption[]>>();

function loadModels(provider: AgentProvider): Promise<ModelOption[]> {
  let pending = cache.get(provider);
  if (!pending) {
    pending = ipc.listProviderModels(provider).catch(() => {
      cache.delete(provider);
      return [];
    });
    cache.set(provider, pending);
  }
  return pending;
}

/**
 * Model dropdown for the selected provider: "Default" (the CLI's own choice), the models the
 * provider offers, and "Custom…" for any other name, which reveals a text field.
 */
export function ModelPicker({
  provider,
  value,
  onChange,
}: {
  provider: AgentProvider;
  value: string;
  onChange: (model: string) => void;
}) {
  const [models, setModels] = useState<ModelOption[] | null>(null);
  const [customOpen, setCustomOpen] = useState(false);

  useEffect(() => {
    let stale = false;
    setModels(null);
    setCustomOpen(false);
    void loadModels(provider).then((list) => {
      if (!stale) setModels(list);
    });
    return () => {
      stale = true;
    };
  }, [provider]);

  const known = models?.some((model) => model.id === value) ?? false;
  // A saved name that is not in the list (an older graph, or a typed name) is shown as custom.
  const custom = customOpen || (models !== null && value !== DEFAULT_VALUE && !known);
  const selected = custom ? CUSTOM_VALUE : value;

  return (
    <div className="space-y-1.5">
      <select
        className={FIELD}
        value={selected}
        disabled={models === null}
        aria-label="Model"
        onChange={(event) => {
          const next = event.target.value;
          if (next === CUSTOM_VALUE) {
            setCustomOpen(true);
            return;
          }
          setCustomOpen(false);
          onChange(next);
        }}
      >
        <option value={DEFAULT_VALUE}>{models === null ? "Loading models…" : "Default (CLI's choice)"}</option>
        {(models ?? []).map((model) => (
          <option key={model.id} value={model.id}>
            {model.label}
            {model.note ? ` — ${model.note}` : ""}
          </option>
        ))}
        <option value={CUSTOM_VALUE}>Custom…</option>
      </select>
      {custom ? (
        <input
          className={FIELD}
          value={value}
          placeholder="model name, passed to the CLI as-is"
          aria-label="Custom model name"
          autoFocus={customOpen && value === ""}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : null}
      {models !== null && models.length === 0 ? (
        <p className="text-[11px] text-zinc-500">No model list is available for this provider. Use Default or Custom.</p>
      ) : null}
    </div>
  );
}
