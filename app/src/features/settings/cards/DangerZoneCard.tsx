import { useEffect, useState } from 'react';

import * as api from '../../../api/commands';
import { Button } from '../../../components/Button';
import { Card, SettingRow } from '../../../components/Card';
import { ConfirmPanel } from '../../../components/ConfirmPanel';
import { useDaemonCommand } from '../../../hooks/useDaemonCommand';
import { useDaemon } from '../../../state/DaemonContext';

export function DangerZoneCard() {
  const { status } = useDaemon();
  const command = useDaemonCommand();
  const [confirming, setConfirming] = useState(false);

  const signedIn = !!status?.account;
  useEffect(() => {
    if (!signedIn) setConfirming(false);
  }, [signedIn]);

  return (
    <Card title="Dangerous" danger>
      <SettingRow
        title="Redownload everything"
        hint="Delete all files in your Google Drive folder and download the whole Drive again from scratch. You stay signed in."
      >
        <Button size="sm" variant="danger-outline" disabled={!signedIn} onClick={() => setConfirming(true)}>
          Redownload…
        </Button>
      </SettingRow>

      {confirming && (
        <ConfirmPanel
          confirmLabel="Delete and redownload"
          onCancel={() => setConfirming(false)}
          onConfirm={async () => {
            await command(api.redownload(), 'Folder cleared — downloading everything again');
            setConfirming(false);
          }}
        >
          Permanently delete everything in <b className="selectable">{status?.sync_root || 'your Google Drive folder'}</b>{' '}
          and download it all again? Changes that have not been uploaded to Drive yet will be lost.
        </ConfirmPanel>
      )}
    </Card>
  );
}
