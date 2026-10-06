import { useEffect, useRef } from 'react';

import { onWindowShown } from '../api/commands';

/** Calls `handler` whenever the window is shown from the tray. */
export function useWindowShown(handler: () => void) {
  const latest = useRef(handler);
  useEffect(() => {
    latest.current = handler;
  });

  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    onWindowShown(() => latest.current()).then(
      (fn) => (cancelled ? fn() : (unlisten = fn)),
      () => {},
    );
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
}
