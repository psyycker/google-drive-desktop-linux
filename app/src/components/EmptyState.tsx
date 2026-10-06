import type { ReactNode } from 'react';

import { Icon, type IconName } from './Icon';
import './EmptyState.css';

export function EmptyState({ icon, title, children }: { icon: IconName; title: string; children: ReactNode }) {
  return (
    <div className="empty">
      <div className="empty-art">
        <Icon name={icon} />
      </div>
      <div className="empty-title">{title}</div>
      <div className="empty-sub">{children}</div>
    </div>
  );
}
