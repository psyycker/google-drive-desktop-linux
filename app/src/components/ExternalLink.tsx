import type { ReactNode } from 'react';

import * as api from '../api/commands';
import { useToast } from '../state/ToastContext';

/** A link that opens in the default browser; the webview itself never navigates. */
export function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  const toast = useToast();
  return (
    <a
      href={href}
      onClick={(e) => {
        e.preventDefault();
        toast.fire(api.openUrl(href));
      }}
    >
      {children}
    </a>
  );
}
