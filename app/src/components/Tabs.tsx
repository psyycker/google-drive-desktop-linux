import type { ReactNode } from 'react';

import { cx } from '../lib/classNames';
import './Tabs.css';

export interface TabDef<Id extends string> {
  id: Id;
  label: ReactNode;
}

interface TabsProps<Id extends string> {
  tabs: TabDef<Id>[];
  active: Id;
  onSelect: (id: Id) => void;
}

export function Tabs<Id extends string>({ tabs, active, onSelect }: TabsProps<Id>) {
  return (
    <nav className="tabs" role="tablist">
      {tabs.map((t) => (
        <button
          key={t.id}
          type="button"
          role="tab"
          aria-selected={t.id === active}
          className={cx('tab', t.id === active && 'active')}
          onClick={() => onSelect(t.id)}
        >
          {t.label}
        </button>
      ))}
    </nav>
  );
}

/** A red counter, hidden at zero. */
export function Badge({ count }: { count: number }) {
  if (count === 0) return null;
  return <span className="badge">{count > 99 ? '99+' : count}</span>;
}
