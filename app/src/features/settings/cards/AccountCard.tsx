import { useEffect, useState } from 'react';

import * as api from '../../../api/commands';
import { Button } from '../../../components/Button';
import { Card, SettingRow } from '../../../components/Card';
import { ConfirmPanel } from '../../../components/ConfirmPanel';
import { useDaemonCommand } from '../../../hooks/useDaemonCommand';
import { useDaemon } from '../../../state/DaemonContext';
import { useSettings } from '../../../state/SettingsContext';
import { useSignIn } from '../../../state/SignInContext';
import { LoginUrlNotice } from '../../signin/LoginUrlNotice';

export function AccountCard() {
  const { status } = useDaemon();
  const { config } = useSettings();
  const { loginUrl, signIn } = useSignIn();
  const command = useDaemonCommand();
  const [confirmSignOut, setConfirmSignOut] = useState(false);

  const account = status?.account ?? null;
  const signedIn = !!account;
  useEffect(() => {
    if (!signedIn) setConfirmSignOut(false);
  }, [signedIn]);

  const canSignIn = !!status && !!config?.client_id && !!config.client_secret;

  return (
    <Card title="Account">
      <SettingRow
        title={
          <span className="muted">
            {account ? (
              <>
                Signed in as <b className="selectable">{account.email}</b>
              </>
            ) : status ? (
              'Not signed in'
            ) : (
              'Sync service not running'
            )}
          </span>
        }
      >
        {account ? (
          <Button size="sm" onClick={() => setConfirmSignOut(true)}>
            Sign out
          </Button>
        ) : (
          <Button variant="primary" size="sm" disabled={!canSignIn} onClick={signIn}>
            Sign in
          </Button>
        )}
      </SettingRow>

      {confirmSignOut && (
        <ConfirmPanel
          confirmLabel="Sign out"
          onCancel={() => setConfirmSignOut(false)}
          onConfirm={async () => {
            await command(api.signOut(), 'Signed out');
            setConfirmSignOut(false);
          }}
        >
          Sign out of this account? Syncing stops; files already in your folder stay on this computer.
        </ConfirmPanel>
      )}

      {loginUrl && <LoginUrlNotice url={loginUrl} />}

      <SettingRow
        divided
        title="Full resync"
        hint="Re-list all of Drive and rescan the folder. Use if something looks out of sync."
      >
        <Button size="sm" disabled={!signedIn} onClick={() => command(api.fullResync(), 'Full resync started')}>
          Resync
        </Button>
      </SettingRow>
    </Card>
  );
}
