import type { ActivityKind } from '../../api/types';
import type { IconName } from '../../components/Icon';
import type { ItemTone } from '../../components/ListItem';

interface KindInfo {
  icon: IconName;
  tone?: ItemTone;
  verb: string;
}

const KINDS: Record<ActivityKind, KindInfo> = {
  uploaded: { icon: 'upload', tone: 'up', verb: 'Uploaded' },
  downloaded: { icon: 'download', tone: 'down', verb: 'Downloaded' },
  created_folder_local: { icon: 'folderPlus', tone: 'down', verb: 'Folder created on this computer' },
  created_folder_remote: { icon: 'folderPlus', tone: 'up', verb: 'Folder created in Drive' },
  moved_local: { icon: 'move', verb: 'Moved on this computer' },
  moved_remote: { icon: 'move', verb: 'Moved in Drive' },
  deleted_local: { icon: 'trash', verb: 'Removed from this computer' },
  deleted_remote: { icon: 'trash', verb: 'Removed from Drive' },
  conflict: { icon: 'conflict', tone: 'warn', verb: 'Conflict — both versions kept' },
};

/** How an activity is shown; kinds added by a newer daemon fall back to a plain row. */
export const kindInfo = (kind: string): KindInfo => KINDS[kind as ActivityKind] ?? { icon: 'cloud', verb: kind };
