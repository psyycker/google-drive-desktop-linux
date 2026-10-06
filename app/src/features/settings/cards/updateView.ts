import type { Status } from '../../../api/types';
import { compareVersions, formatBytes } from '../../../lib/format';

export type UpdateAction = { kind: 'restart'; label: string } | { kind: 'open'; label: string; url: string };

export interface UpdateView {
  line: string;
  sub: string;
  action: UpdateAction | null;
  /** Download progress (0–1), or `null` to hide the bar. */
  progress: number | null;
}

/** What the Updates card says, from the daemon's update state and both versions. */
export function updateView(status: Status | null, appVersion: string | null): UpdateView {
  const running = status?.version || null;
  const u = status?.update ?? null;

  // The daemon updated itself; this window still runs the old app.
  if (appVersion && running && compareVersions(running, appVersion) > 0) {
    return {
      line: `Updated to version ${running}`,
      sub: 'Restart the app to finish updating. It restarts by itself next time this window is closed.',
      action: { kind: 'restart', label: 'Restart now' },
      progress: null,
    };
  }

  if (!u) {
    return {
      line: `Version ${appVersion || running || '…'}`,
      sub: status ? 'You have the latest version.' : '',
      action: null,
      progress: null,
    };
  }

  const download: UpdateAction = { kind: 'open', label: 'Download', url: u.release_url };
  switch (u.phase) {
    case 'available':
      return {
        line: `Version ${u.latest} is available`,
        sub: u.automatic ? 'It will install automatically once syncing is idle.' : 'Download it from the release page.',
        action: u.automatic ? null : download,
        progress: null,
      };
    case 'downloading': {
      const known = u.bytes_total > 0;
      return {
        line: `Downloading version ${u.latest}…`,
        sub: known ? `${formatBytes(u.bytes_done)} of ${formatBytes(u.bytes_total)}` : '',
        action: null,
        progress: known ? u.bytes_done / u.bytes_total : null,
      };
    }
    case 'installing':
      return { line: `Installing version ${u.latest}…`, sub: u.message ?? '', action: null, progress: null };
    default:
      return { line: `Couldn’t install version ${u.latest}`, sub: u.message ?? '', action: download, progress: null };
  }
}
