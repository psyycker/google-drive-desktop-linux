import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import * as api from '../api/commands';
import type { Status } from '../api/types';
import { useWindowShown } from '../hooks/useWindowShown';
import { createStrictContext } from './strictContext';

/** Poll interval while the window is visible; the tray keeps its own, slower poll. */
const POLL_MS = 1000;

export interface DaemonApi {
  /** Last status from gdrived, or `null` while it is unreachable. */
  status: Status | null;
  /** False until the first status request has finished. */
  loaded: boolean;
  /** This app's own version (the daemon may be newer after an update). */
  appVersion: string | null;
  /** Fetches the status now (no-op while a request is already in flight). */
  refresh(): Promise<void>;
}

const [Provider, useDaemon] = createStrictContext<DaemonApi>('Daemon');
export { useDaemon };

export function DaemonProvider({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      setStatus(await api.getStatus());
    } catch {
      setStatus(null);
    } finally {
      inFlight.current = false;
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    api.appVersion().then(setAppVersion, () => {});
  }, []);

  useEffect(() => {
    void refresh();
    const tick = () => {
      if (!document.hidden) void refresh();
    };
    const id = window.setInterval(tick, POLL_MS);
    document.addEventListener('visibilitychange', tick);
    return () => {
      window.clearInterval(id);
      document.removeEventListener('visibilitychange', tick);
    };
  }, [refresh]);

  useWindowShown(refresh);

  const value = useMemo(() => ({ status, loaded, appVersion, refresh }), [status, loaded, appVersion, refresh]);
  return <Provider value={value}>{children}</Provider>;
}
