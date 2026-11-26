import { useCallback, useEffect, useState } from "react";

export type ThemeChoice = "system" | "light" | "dark";

export type Preferences = {
  theme: ThemeChoice;
  displayName: string;
};

const STORAGE_KEY = "openconductor.preferences";
const DEFAULTS: Preferences = { theme: "dark", displayName: "" };

function readPreferences(): Preferences {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return DEFAULTS;
    const parsed = JSON.parse(raw) as Partial<Preferences>;
    const theme = parsed.theme === "light" || parsed.theme === "dark" || parsed.theme === "system" ? parsed.theme : DEFAULTS.theme;
    return { theme, displayName: typeof parsed.displayName === "string" ? parsed.displayName : DEFAULTS.displayName };
  } catch {
    return DEFAULTS;
  }
}

function resolveTheme(choice: ThemeChoice): "light" | "dark" {
  if (choice !== "system") return choice;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

/**
 * Locally persisted preferences. The resolved theme is written to `<html data-theme>`, which index.css uses to
 * swap the palette. Storage failures fall back to in-memory state for the session.
 */
export function usePreferences() {
  const [preferences, setPreferences] = useState<Preferences>(readPreferences);

  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(preferences));
    } catch {
      // Storage unavailable: keep the preference for this session only.
    }
  }, [preferences]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      document.documentElement.dataset.theme = resolveTheme(preferences.theme);
    };
    apply();
    if (preferences.theme !== "system") return;
    // Follow the OS while the "System" choice is selected.
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [preferences.theme]);

  const update = useCallback((patch: Partial<Preferences>) => {
    setPreferences((current) => ({ ...current, ...patch }));
  }, []);

  return { preferences, update };
}
