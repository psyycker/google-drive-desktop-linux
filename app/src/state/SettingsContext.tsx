import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';

import * as api from '../api/commands';
import type { Config } from '../api/types';
import { useWindowShown } from '../hooks/useWindowShown';
import { errorMessage } from '../lib/format';
import { useDaemon } from './DaemonContext';
import { configFromForm, EMPTY_FORM, formFromConfig, type SettingsForm } from './settingsForm';
import { createStrictContext } from './strictContext';
import { useToast } from './ToastContext';

export interface SettingsApi {
  /** The configuration as last loaded from, or saved to, the daemon. */
  config: Config | null;
  /** The form's current values, possibly with unsaved edits. */
  form: SettingsForm;
  /** True while `form` has edits that are not saved. */
  dirty: boolean;
  /** Changes form fields and marks the form dirty. */
  edit(patch: Partial<SettingsForm>): void;
  /** Throws away unsaved edits. */
  discard(): void;
  /** Reloads the configuration, unless there are unsaved edits. */
  reloadIfClean(): void;
  /** Validates and saves the form; toasts and rethrows on failure. */
  save(success?: string): Promise<void>;
}

const [Provider, useSettings] = createStrictContext<SettingsApi>('Settings');
export { useSettings };

/**
 * Holds the one settings form. Settings and onboarding both edit it, so the
 * credentials and folder typed during onboarding are the ones Settings shows.
 */
export function SettingsProvider({ children }: { children: ReactNode }) {
  const toast = useToast();
  const { refresh } = useDaemon();
  const [config, setConfig] = useState<Config | null>(null);
  const [form, setForm] = useState<SettingsForm>(EMPTY_FORM);
  const [dirty, setDirtyState] = useState(false);
  // Mirrors `dirty` for async code and event handlers that would see a stale value.
  const dirtyRef = useRef(false);

  const setDirty = useCallback((v: boolean) => {
    dirtyRef.current = v;
    setDirtyState(v);
  }, []);

  const load = useCallback(async () => {
    try {
      const loaded = await api.getConfig();
      // Edits made while the request was in flight win over the reloaded values.
      if (dirtyRef.current) return;
      setConfig(loaded);
      setForm(formFromConfig(loaded));
    } catch (e) {
      toast.error(`Couldn’t load settings: ${errorMessage(e)}`);
    }
  }, [toast]);

  const reloadIfClean = useCallback(() => {
    if (!dirtyRef.current) void load();
  }, [load]);

  useEffect(() => {
    void load();
  }, [load]);
  useWindowShown(reloadIfClean);

  const edit = useCallback(
    (patch: Partial<SettingsForm>) => {
      setForm((f) => ({ ...f, ...patch }));
      setDirty(true);
    },
    [setDirty],
  );

  const discard = useCallback(() => {
    if (config) setForm(formFromConfig(config));
    setDirty(false);
  }, [config, setDirty]);

  const save = useCallback(
    async (success = 'Settings saved') => {
      let next: Config;
      try {
        next = configFromForm(form, config);
      } catch (e) {
        toast.error(e);
        throw e;
      }
      await toast.run(api.setConfig(next), success);
      setConfig(next);
      setDirty(false);
      await load();
      void refresh();
    },
    [form, config, toast, setDirty, load, refresh],
  );

  const value = useMemo(
    () => ({ config, form, dirty, edit, discard, reloadIfClean, save }),
    [config, form, dirty, edit, discard, reloadIfClean, save],
  );
  return <Provider value={value}>{children}</Provider>;
}
