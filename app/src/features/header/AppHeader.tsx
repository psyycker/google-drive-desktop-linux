import * as api from '../../api/commands';
import type { Account } from '../../api/types';
import { Icon } from '../../components/Icon';
import { IconButton } from '../../components/IconButton';
import { useSyncRoot } from '../../hooks/useSyncRoot';
import { hueFor } from '../../lib/format';
import { useDaemon } from '../../state/DaemonContext';
import { useToast } from '../../state/ToastContext';
import { QuotaBar } from './QuotaBar';
import './AppHeader.css';

const DRIVE_WEB_URL = 'https://drive.google.com';

/** Account, storage quota and shortcuts to the local folder and the web. */
export function AppHeader() {
  const { status, loaded } = useDaemon();
  const root = useSyncRoot();
  const toast = useToast();
  const account = status?.account ?? null;

  let name = 'Google Drive';
  let detail: string;
  if (account) {
    name = account.display_name || account.email;
    detail = account.display_name ? account.email : 'Google Drive';
  } else if (!loaded) {
    detail = 'Connecting…';
  } else {
    detail = status ? 'Not signed in' : 'Sync service not running';
  }

  return (
    <>
      <header className="header">
        <Avatar account={account} />
        <div className="account">
          <div className="account-name">{name}</div>
          <div className="account-email">{detail}</div>
        </div>
        <div className="header-actions">
          <IconButton
            icon="folder"
            label="Open Google Drive folder"
            disabled={!root}
            onClick={() => toast.fire(api.openPath(root))}
          />
          <IconButton
            icon="globe"
            label="Open Google Drive on the web"
            onClick={() => toast.fire(api.openUrl(DRIVE_WEB_URL))}
          />
        </div>
      </header>
      {account && status?.quota && <QuotaBar quota={status.quota} />}
    </>
  );
}

/** The account's initial on a colour derived from its email, or a cloud when signed out. */
function Avatar({ account }: { account: Account | null }) {
  if (!account) {
    return (
      <div className="avatar" aria-hidden="true">
        <Icon name="cloud" />
      </div>
    );
  }
  const label = account.display_name || account.email;
  return (
    <div className="avatar" aria-hidden="true" style={{ background: `hsl(${hueFor(account.email)} 55% 48%)` }}>
      {(label.trim()[0] ?? '?').toUpperCase()}
    </div>
  );
}
