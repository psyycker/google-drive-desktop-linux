// Mirrors the serde types in crates/gdrive-core/src/status.rs and config.rs.
// Timestamps arrive as RFC 3339 strings.

export type SyncState =
  | 'setup_required'
  | 'signed_out'
  | 'signing_in'
  | 'starting'
  | 'idle'
  | 'syncing'
  | 'paused'
  | 'offline'
  | 'error';

export interface Account {
  email: string;
  display_name: string;
  photo_link: string | null;
}

export interface Quota {
  used: number;
  /** `null` for unlimited plans. */
  limit: number | null;
}

export type Direction = 'upload' | 'download';

export interface Transfer {
  path: string;
  direction: Direction;
  bytes_done: number;
  bytes_total: number;
}

export type ActivityKind =
  | 'uploaded'
  | 'downloaded'
  | 'created_folder_local'
  | 'created_folder_remote'
  | 'moved_local'
  | 'moved_remote'
  | 'deleted_local'
  | 'deleted_remote'
  | 'conflict';

export interface Activity {
  time: string;
  kind: ActivityKind;
  /** Path relative to the sync root. */
  path: string;
  detail: string | null;
}

export interface ItemError {
  time: string;
  path: string;
  message: string;
}

export type UpdatePhase = 'available' | 'downloading' | 'installing' | 'failed';

export interface UpdateInfo {
  latest: string;
  phase: UpdatePhase;
  /** True when this install can update itself; otherwise the user updates manually. */
  automatic: boolean;
  release_url: string;
  message: string | null;
  bytes_done: number;
  bytes_total: number;
}

/** Snapshot of the daemon's state. */
export interface Status {
  state: SyncState;
  /** Human-readable explanation for error/offline/signing-in states. */
  message: string | null;
  account: Account | null;
  quota: Quota | null;
  sync_root: string;
  /** Number of items waiting to be reconciled. */
  pending: number;
  transfers: Transfer[];
  download_bps: number;
  upload_bps: number;
  /** Newest first. */
  recent: Activity[];
  /** Newest first. */
  errors: ItemError[];
  last_synced: string | null;
  /** Version of the running daemon. */
  version: string;
  update: UpdateInfo | null;
  /** Set while a folder is being moved to or from another drive. */
  relocation: Relocation | null;
  /** How many Drive folders are selected for syncing; `null` when the whole Drive syncs. */
  selected_folders: number | null;
}

export interface Relocation {
  /** Drive-relative folder being moved. */
  folder: string;
  /** Storage location name, or `null` when moving back to the main drive. */
  to: string | null;
  bytes_done: number;
  /** 0 until known (and for a rename on the same disk). */
  bytes_total: number;
}

/** A folder on another disk that can hold some Drive folders. */
export interface StorageLocation {
  name: string;
  root: string;
}

/** A Drive folder kept on a storage location instead of the main Drive folder. */
export interface Placement {
  /** Relative to the Drive root, e.g. `Gaming`. */
  folder: string;
  location: string;
}

/** A Drive folder chosen for syncing. */
export interface SelectedFolder {
  /** Drive id; the choice follows the folder when it is renamed or moved. */
  id: string;
  /** Relative to the Drive root when last looked up, e.g. `Work/Reports`. */
  path: string;
}

/** How much of a Drive folder is synced. */
export type Coverage = 'whole' | 'partly' | 'unsynced';

/** A folder on Drive, as listed for choosing what to sync. */
export interface FolderEntry {
  /** Its name on Drive. */
  name: string;
  /** Relative to the Drive root, spelled as on disk. */
  path: string;
  coverage: Coverage;
}

/** User configuration (`~/.config/gdrive-linux/config.toml`). */
export interface Config {
  client_id: string;
  client_secret: string;
  sync_root: string;
  poll_interval_secs: number;
  max_concurrent_transfers: number;
  ignore: string[];
  use_local_trash: boolean;
  /** MB/s across all transfers; 0 = unlimited. */
  max_download_mb_per_sec: number;
  /** MB/s across all transfers; 0 = unlimited. */
  max_upload_mb_per_sec: number;
  auto_update: boolean;
  locations: StorageLocation[];
  /** Changed only by moving folders (`relocateFolder`); the daemon ignores edits. */
  placements: Placement[];
  /** The Drive folders synced; `null` syncs the whole Drive. Changed only through the
   * daemon (`gdrive folders`); it ignores edits here. */
  selected_folders: SelectedFolder[] | null;
}
