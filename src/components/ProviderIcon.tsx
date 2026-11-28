import { PROVIDER_LABELS, type AgentProvider } from "../types/orchestration";
import { PROVIDER_MONOGRAM } from "../lib/usage";
import { PROVIDER_ACCENT, PROVIDER_LOGO } from "./providerLogos";

const SIZES = {
  sm: { badge: "h-5 w-5 rounded-md", mark: "h-3 w-3", letter: "text-[9px]" },
  md: { badge: "h-8 w-8 rounded-lg", mark: "h-5 w-5", letter: "text-[11px]" },
} as const;

/**
 * The provider's logo on a dark badge, or its letter on the accent colour when no logo is
 * available. Decorative by default, because the provider name is always written beside it.
 */
export function ProviderIcon({ provider, size = "md" }: { provider: AgentProvider; size?: keyof typeof SIZES }) {
  const { badge, mark, letter } = SIZES[size];
  const logo = PROVIDER_LOGO[provider];
  if (logo) {
    return (
      <span
        data-provider-icon={provider}
        aria-hidden
        className={`flex shrink-0 items-center justify-center bg-zinc-800 ring-1 ring-zinc-700 ${badge}`}
      >
        <svg viewBox="0 0 24 24" className={mark} fill={logo.color} role="presentation" focusable="false">
          <path d={logo.path} />
        </svg>
      </span>
    );
  }
  return (
    <span
      data-provider-icon={provider}
      aria-hidden
      title={PROVIDER_LABELS[provider]}
      className={`flex shrink-0 items-center justify-center font-bold text-zinc-950 ${badge} ${letter} ${PROVIDER_ACCENT[provider]}`}
    >
      {PROVIDER_MONOGRAM[provider]}
    </span>
  );
}
