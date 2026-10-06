import { EmptyState } from '../../components/EmptyState';
import { useDaemon } from '../../state/DaemonContext';
import { RecentList } from './RecentList';
import { TransferList } from './TransferList';

export function ActivityPanel() {
  const { status } = useDaemon();
  const transfers = status?.transfers ?? [];
  const recent = status?.recent ?? [];

  return (
    <>
      <TransferList transfers={transfers} />
      <RecentList activities={recent} />
      {status && !transfers.length && !recent.length && (
        <EmptyState icon="cloudCheck" title="No recent activity">
          Files you add or change in your Google Drive folder will show up here.
        </EmptyState>
      )}
    </>
  );
}
