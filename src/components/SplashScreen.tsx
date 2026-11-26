import { useEffect, useState } from "react";
import { CondurenceMark } from "./CondurenceMark";

/** The splash starts fading at this point, and is removed once the fade has finished. */
const FADE_OUT_AT_MS = 1550;
const REMOVE_AT_MS = 1900;

/** Opening splash: the mark draws in and the name rises, then the window fades to the app. Runs once per launch. */
export function SplashScreen() {
  const [fading, setFading] = useState(false);
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    const fade = window.setTimeout(() => setFading(true), FADE_OUT_AT_MS);
    const remove = window.setTimeout(() => setVisible(false), REMOVE_AT_MS);
    return () => {
      window.clearTimeout(fade);
      window.clearTimeout(remove);
    };
  }, []);

  if (!visible) return null;

  return (
    <div
      role="status"
      aria-label="Starting Condurence"
      className={`fixed inset-0 z-50 flex flex-col items-center justify-center gap-6 bg-zinc-950 transition-opacity duration-300 ${
        fading ? "pointer-events-none opacity-0" : "opacity-100"
      }`}
    >
      <CondurenceMark animate className="size-32" />
      <div className="oc-splash-rise text-center">
        <div className="text-2xl font-semibold tracking-tight text-zinc-100">Condurence</div>
        <div className="mt-1 text-xs text-zinc-500">Agentic development environment</div>
      </div>
    </div>
  );
}
