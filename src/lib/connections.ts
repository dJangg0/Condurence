import { useCallback, useEffect, useState } from "react";
import { ipc } from "./ipc";
import type { GitHubAccount } from "../types/github";
import type { ProviderAuth } from "../types/providers";

export type Connections = {
  github: GitHubAccount | null;
  providers: ProviderAuth[] | null;
  error: string | null;
  refresh: () => void;
};

/** Sign-in state for GitHub and the agent CLIs. Loads on mount; call `refresh` after a sign-in. */
export function useConnections(): Connections {
  const [github, setGithub] = useState<GitHubAccount | null>(null);
  const [providers, setProviders] = useState<ProviderAuth[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);

  useEffect(() => {
    let stale = false;
    ipc
      .githubAccountStatus()
      .then((next) => !stale && setGithub(next))
      .catch((e: unknown) => !stale && setError(String(e)));
    ipc
      .providerAuthStatus()
      .then((next) => !stale && setProviders(next))
      .catch((e: unknown) => !stale && setError(String(e)));
    return () => {
      stale = true;
    };
  }, [revision]);

  const refresh = useCallback(() => setRevision((n) => n + 1), []);
  return { github, providers, error, refresh };
}
