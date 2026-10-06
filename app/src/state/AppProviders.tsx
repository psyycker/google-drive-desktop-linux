import type { ReactNode } from 'react';

import { DaemonProvider } from './DaemonContext';
import { SettingsProvider } from './SettingsContext';
import { SignInProvider } from './SignInContext';
import { ToastProvider } from './ToastContext';

/** All app-wide state, outermost first: each provider may use the ones around it. */
export function AppProviders({ children }: { children: ReactNode }) {
  return (
    <ToastProvider>
      <DaemonProvider>
        <SettingsProvider>
          <SignInProvider>{children}</SignInProvider>
        </SettingsProvider>
      </DaemonProvider>
    </ToastProvider>
  );
}
