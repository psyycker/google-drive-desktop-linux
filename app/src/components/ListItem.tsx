import type { ReactNode } from 'react';

import { cx } from '../lib/classNames';
import { Icon, type IconName } from './Icon';
import { RelativeTime } from './RelativeTime';
import './ListItem.css';

export type ItemTone = 'up' | 'down' | 'warn' | 'danger';

interface ListItemProps {
  icon: IconName;
  tone?: ItemTone;
  name: string;
  sub: ReactNode;
  /** Tooltip for the whole row. */
  title?: string;
  /** Shown between the name and the subtitle. */
  progress?: ReactNode;
  /** Shown under the subtitle. */
  children?: ReactNode;
  /** RFC 3339 timestamp shown on the right. */
  time?: string;
  onClick?: () => void;
}

/** A row in the activity and error lists: icon, name, subtitle and time. */
export function ListItem({ icon, tone, name, sub, title, progress, children, time, onClick }: ListItemProps) {
  return (
    <div className="item" title={title} onClick={onClick}>
      <div className={cx('item-icon', tone)}>
        <Icon name={icon} />
      </div>
      <div className="item-main">
        <div className="item-name">{name}</div>
        {progress}
        <div className="item-sub">{sub}</div>
        {children}
      </div>
      {time && (
        <div className="item-time">
          <RelativeTime iso={time} />
        </div>
      )}
    </div>
  );
}

export const SectionLabel = ({ children }: { children: ReactNode }) => <div className="section-label">{children}</div>;
