import type { SyncState } from '../api/types';

/** States in which the window shows the setup steps instead of the main view. */
export const isOnboardingState = (s: SyncState) => s === 'setup_required' || s === 'signed_out' || s === 'signing_in';

/** States in which syncing can be paused, resumed or triggered. */
export const isActiveState = (s: SyncState) =>
  s === 'starting' || s === 'idle' || s === 'syncing' || s === 'paused' || s === 'offline' || s === 'error';
