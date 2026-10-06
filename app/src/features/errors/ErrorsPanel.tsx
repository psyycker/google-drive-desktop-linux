import { EmptyState } from '../../components/EmptyState';
import { ListItem } from '../../components/ListItem';
import { useOpenContainingFolder } from '../../hooks/useSyncRoot';
import { baseName, locationLabel } from '../../lib/paths';
import { useDaemon } from '../../state/DaemonContext';
import './ErrorsPanel.css';

/** Items that failed to sync. Clicking a row opens its folder. */
export function ErrorsPanel() {
  const { status } = useDaemon();
  const openFolder = useOpenContainingFolder();
  const errors = status?.errors ?? [];

  if (status && !errors.length) {
    return (
      <EmptyState icon="checkCircle" title="No sync problems">
        Items that fail to sync are listed here.
      </EmptyState>
    );
  }

  return errors.map((e) => (
    <ListItem
      // The daemon keeps one entry per path.
      key={e.path}
      icon="warning"
      tone="danger"
      name={baseName(e.path) || 'Google Drive'}
      title={e.path}
      sub={locationLabel(e.path)}
      time={e.time}
      onClick={() => openFolder(e.path)}
    >
      <div className="error-msg">{e.message}</div>
    </ListItem>
  ));
}
