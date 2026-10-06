import { useCallback } from 'react';

import { useDaemon } from '../state/DaemonContext';
import { useToast } from '../state/ToastContext';

/**
 * Returns a function that awaits a daemon command, toasts `success` (or the error, then
 * rethrows), and refreshes the status so the window reflects the change right away.
 */
export function useDaemonCommand() {
  const toast = useToast();
  const { refresh } = useDaemon();
  return useCallback(
    async (task: Promise<unknown>, success?: string) => {
      await toast.run(task, success);
      await refresh();
    },
    [toast, refresh],
  );
}
