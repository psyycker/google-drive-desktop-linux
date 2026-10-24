// Typed wrappers around the Tauri commands in app/src-tauri/src/commands.rs.
// Commands reject with the error text (a string) produced on the Rust side.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';

import type { Config, FolderEntry, SelectedFolder, Status } from './types';

export const getStatus = () => invoke<Status>('get_status');

export const pause = () => invoke<void>('pause');
export const resume = () => invoke<void>('resume');
export const syncNow = () => invoke<void>('sync_now');
export const fullResync = () => invoke<void>('full_resync');
export const redownload = () => invoke<void>('redownload');
/** Moves a Drive folder to a storage location, or back to the main drive with `null`. */
export const relocateFolder = (folder: string, location: string | null) =>
  invoke<void>('relocate_folder', { folder, location });

/** The Drive folders chosen for syncing; `null` when the whole Drive syncs. */
export const getFolders = () => invoke<SelectedFolder[] | null>('get_folders');
/** The folders directly inside a Drive folder (`''` for the Drive root). */
export const browseFolders = (path: string) => invoke<FolderEntry[]>('browse_folders', { path });
/** Syncs only these Drive folders, or the whole Drive with `null`. */
export const setFolders = (folders: string[] | null) => invoke<void>('set_folders', { folders });

/** Starts the OAuth flow; resolves to the URL in case the browser did not open. */
export const startLogin = () => invoke<string>('start_login');
export const signOut = () => invoke<void>('sign_out');

export const getConfig = () => invoke<Config>('get_config');
export const setConfig = (config: Config) => invoke<void>('set_config', { config });

/** Opens a file or folder (or its nearest existing parent) with the default handler. */
export const openPath = (path: string) => invoke<void>('open_path', { path });
/** Opens an http(s) URL in the default browser. */
export const openUrl = (url: string) => invoke<void>('open_url', { url });

export const appVersion = () => invoke<string>('app_version');
export const checkForUpdates = () => invoke<void>('check_for_updates');
export const restartApp = () => invoke<void>('restart_app');

export const getAutostart = () => invoke<boolean>('get_autostart');
export const setAutostart = (enabled: boolean) => invoke<void>('set_autostart', { enabled });

/** Asks for a folder; resolves to `null` when the dialog is cancelled. */
export async function pickFolder(
  defaultPath?: string,
  title = 'Choose your Google Drive folder',
): Promise<string | null> {
  const picked = await open({ directory: true, multiple: false, defaultPath, title });
  return typeof picked === 'string' && picked ? picked : null;
}

/** Fired by the shell whenever the window is shown from the tray. */
export const onWindowShown = (handler: () => void): Promise<UnlistenFn> =>
  listen('app://shown', handler);
