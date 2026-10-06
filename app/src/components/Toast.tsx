import type { ReactNode } from 'react';

import { cx } from '../lib/classNames';
import './Toast.css';

/** The floating message at the bottom; shown through `useToast()`. */
export function Toast({ error, children }: { error: boolean; children: ReactNode }) {
  return (
    <div className={cx('toast', error && 'error')} role="status" aria-live="polite">
      {children}
    </div>
  );
}
