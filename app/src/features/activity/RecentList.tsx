import type { Activity } from '../../api/types';
import { ListItem, SectionLabel } from '../../components/ListItem';
import { useOpenContainingFolder } from '../../hooks/useSyncRoot';
import { baseName, locationLabel } from '../../lib/paths';
import { kindInfo } from './activityKinds';

/** What was synced recently, newest first. Clicking a row opens its folder. */
export function RecentList({ activities }: { activities: Activity[] }) {
  const openFolder = useOpenContainingFolder();
  if (!activities.length) return null;

  return (
    <>
      <SectionLabel>Recent activity</SectionLabel>
      {activities.map((a) => {
        const kind = kindInfo(a.kind);
        return (
          <ListItem
            key={`${a.time}:${a.kind}:${a.path}`}
            icon={kind.icon}
            tone={kind.tone}
            name={baseName(a.path)}
            title={a.detail ? `${a.path}\n${a.detail}` : a.path}
            sub={
              <>
                <span className="verb">{kind.verb}</span> · {locationLabel(a.path)}
                {a.detail && ` · ${a.detail}`}
              </>
            }
            time={a.time}
            onClick={() => openFolder(a.path)}
          />
        );
      })}
    </>
  );
}
