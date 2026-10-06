import type { ReactNode } from 'react';

import type { Status } from '../../api/types';
import type { IconName } from '../../components/Icon';
import { RelativeTime } from '../../components/RelativeTime';
import { plural, speedText } from '../../lib/format';

export type BannerTone = 'neutral' | 'ok' | 'busy' | 'warn' | 'error';

export interface BannerContent {
  tone: BannerTone;
  icon: IconName;
  spin?: boolean;
  title: string;
  sub: ReactNode;
}

/** What the status banner says for a daemon state. */
export function bannerContent(status: Status | null, loaded: boolean): BannerContent {
  if (!status) {
    if (!loaded) return { tone: 'neutral', icon: 'cloud', title: 'Connecting…', sub: '' };
    return {
      tone: 'error',
      icon: 'power',
      title: 'Sync service not running',
      sub: 'The gdrived service could not be started. This window reconnects automatically once it runs.',
    };
  }

  const s = status;
  const items = (n: number) => plural(n, 'item', 'items');
  const changes = (n: number) => plural(n, 'change', 'changes');

  switch (s.state) {
    case 'starting':
      return { tone: 'busy', icon: 'loader', spin: true, title: 'Starting…', sub: 'Connecting to Google Drive' };
    case 'idle': {
      const failed = s.errors.length;
      return {
        tone: failed ? 'warn' : 'ok',
        icon: failed ? 'warning' : 'cloudCheck',
        title: 'Up to date',
        sub: (
          <>
            {failed > 0 && `${items(failed)} couldn’t sync · `}
            {s.last_synced ? (
              <>
                Last synced <RelativeTime iso={s.last_synced} />
              </>
            ) : (
              'Everything is in sync'
            )}
          </>
        ),
      };
    }
    case 'syncing': {
      const n = s.transfers.length;
      const title = n > 0 ? `Syncing ${items(n)}` : s.pending > 0 ? `Syncing ${items(s.pending)}` : 'Syncing…';
      const waiting = s.pending > n ? `${changes(s.pending)} waiting` : 'Keeping your files up to date';
      const speed = speedText(s.download_bps, s.upload_bps);
      return { tone: 'busy', icon: 'refresh', spin: true, title, sub: speed ? `${speed} · ${waiting}` : waiting };
    }
    case 'paused':
      return {
        tone: 'neutral',
        icon: 'pause',
        title: 'Paused',
        sub: s.pending ? `${changes(s.pending)} will sync when you resume` : 'Changes will sync when you resume',
      };
    case 'offline':
      return { tone: 'warn', icon: 'wifiOff', title: 'Offline — retrying', sub: s.message || 'Waiting for a network connection' };
    case 'error':
      return { tone: 'error', icon: 'alert', title: 'Sync error', sub: s.message || 'Syncing stopped because of an error' };
    default:
      return { tone: 'neutral', icon: 'cloud', title: s.message || s.state, sub: '' };
  }
}
