import type { Config } from '../api/types';

/** The settings form's fields, as the inputs hold them (numbers stay strings while edited). */
export interface SettingsForm {
  clientId: string;
  clientSecret: string;
  syncRoot: string;
  pollSecs: string;
  concurrency: string;
  /** MB/s; empty for unlimited. */
  downloadLimit: string;
  /** MB/s; empty for unlimited. */
  uploadLimit: string;
  localTrash: boolean;
  autoUpdate: boolean;
  /** One pattern per line. */
  ignore: string;
}

export const EMPTY_FORM: SettingsForm = {
  clientId: '',
  clientSecret: '',
  syncRoot: '',
  pollSecs: '15',
  concurrency: '4',
  downloadLimit: '',
  uploadLimit: '',
  localTrash: true,
  autoUpdate: true,
  ignore: '',
};

const limitText = (mbps: number) => (mbps > 0 ? String(mbps) : '');

export function formFromConfig(c: Config): SettingsForm {
  return {
    clientId: c.client_id,
    clientSecret: c.client_secret,
    syncRoot: c.sync_root,
    pollSecs: String(c.poll_interval_secs),
    concurrency: String(c.max_concurrent_transfers),
    downloadLimit: limitText(c.max_download_mb_per_sec),
    uploadLimit: limitText(c.max_upload_mb_per_sec),
    localTrash: c.use_local_trash,
    autoUpdate: c.auto_update,
    ignore: c.ignore.join('\n'),
  };
}

function parseInteger(raw: string, min: number, max: number, name: string): number {
  const v = Number.parseInt(raw, 10);
  if (!Number.isFinite(v) || v < min || v > max) throw new Error(`${name} must be between ${min} and ${max}`);
  return v;
}

function parseLimit(raw: string, name: string): number {
  const text = raw.trim();
  if (text === '') return 0;
  const v = Number.parseFloat(text);
  if (!Number.isFinite(v) || v < 0) {
    throw new Error(`${name} must be a positive number of MB/s, or empty for unlimited`);
  }
  return v;
}

/**
 * Validates the form and builds the config to save. Fields the form does not show are
 * kept from `base`. Throws an `Error` with a user-facing message when a field is invalid.
 */
export function configFromForm(form: SettingsForm, base: Config | null): Config {
  const root = form.syncRoot.trim();
  if (!root.startsWith('/')) throw new Error('The Google Drive folder must be an absolute path');
  return {
    ...base,
    client_id: form.clientId.trim(),
    client_secret: form.clientSecret.trim(),
    sync_root: root.length > 1 ? root.replace(/\/+$/, '') : root,
    poll_interval_secs: parseInteger(form.pollSecs, 5, 3600, 'Check interval'),
    max_concurrent_transfers: parseInteger(form.concurrency, 1, 16, 'Parallel transfers'),
    max_download_mb_per_sec: parseLimit(form.downloadLimit, 'Download limit'),
    max_upload_mb_per_sec: parseLimit(form.uploadLimit, 'Upload limit'),
    use_local_trash: form.localTrash,
    auto_update: form.autoUpdate,
    ignore: form.ignore
      .split('\n')
      .map((l) => l.trim())
      .filter(Boolean),
  };
}
