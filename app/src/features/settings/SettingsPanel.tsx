import { useEffect } from 'react';

import { useSettings } from '../../state/SettingsContext';
import { AccountCard } from './cards/AccountCard';
import { CredentialsCard } from './cards/CredentialsCard';
import { DangerZoneCard } from './cards/DangerZoneCard';
import { GeneralCard } from './cards/GeneralCard';
import { SyncCard } from './cards/SyncCard';
import { UpdatesCard } from './cards/UpdatesCard';
import { SaveBar } from './SaveBar';
import './SettingsPanel.css';

export function SettingsPanel() {
  const { dirty, reloadIfClean } = useSettings();

  // Pick up changes made elsewhere (e.g. with the CLI) each time the tab opens.
  useEffect(() => {
    reloadIfClean();
  }, [reloadIfClean]);

  return (
    <div className="settings">
      <AccountCard />
      <SyncCard />
      <CredentialsCard />
      <GeneralCard />
      <UpdatesCard />
      <DangerZoneCard />
      {dirty && <SaveBar />}
    </div>
  );
}
