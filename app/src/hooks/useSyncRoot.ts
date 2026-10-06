import { useCallback } from 'react';

import * as api from '../api/commands';
import { joinRoot, parentOf } from '../lib/paths';
import { useDaemon } from '../state/DaemonContext';
import { useSettings } from '../state/SettingsContext';
import { useToast } from '../state/ToastContext';

/** The local Drive folder: the daemon's, or the configured one while it is unreachable. */
export function useSyncRoot(): string {
  const { status } = useDaemon();
  const { config } = useSettings();
  return status?.sync_root || config?.sync_root || '';
}

/** Returns a function that opens the local folder containing a Drive-relative path. */
export function useOpenContainingFolder() {
  const root = useSyncRoot();
  const toast = useToast();
  return useCallback(
    (relPath: string) => {
      if (root) toast.fire(api.openPath(joinRoot(root, parentOf(relPath))));
    },
    [root, toast],
  );
}
