import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react';

import * as api from '../api/commands';
import { useDaemon } from './DaemonContext';
import { useSettings } from './SettingsContext';
import { createStrictContext } from './strictContext';
import { useToast } from './ToastContext';

export interface SignInApi {
  /** The sign-in link to offer in case the browser did not open, or `null`. */
  loginUrl: string | null;
  /** Saves pending settings, then starts the OAuth flow in the browser. */
  signIn(): Promise<void>;
}

const [Provider, useSignIn] = createStrictContext<SignInApi>('SignIn');
export { useSignIn };

export function SignInProvider({ children }: { children: ReactNode }) {
  const { status, refresh } = useDaemon();
  const { dirty, save } = useSettings();
  const toast = useToast();
  const [url, setUrl] = useState<string | null>(null);

  const signedIn = !!status?.account;
  useEffect(() => {
    if (signedIn) setUrl(null);
  }, [signedIn]);

  const signIn = useCallback(async () => {
    if (dirty) await save();
    setUrl(await toast.run(api.startLogin()));
    void refresh();
  }, [dirty, save, toast, refresh]);

  const waiting = status?.state === 'signing_in' || status?.state === 'signed_out';
  const loginUrl = waiting ? url : null;

  const value = useMemo(() => ({ loginUrl, signIn }), [loginUrl, signIn]);
  return <Provider value={value}>{children}</Provider>;
}
