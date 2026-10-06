import type { ReactNode } from 'react';

import { cx } from '../lib/classNames';
import './Card.css';

interface CardProps {
  title: string;
  /** Red outline and heading, for destructive settings. */
  danger?: boolean;
  className?: string;
  children: ReactNode;
}

export function Card({ title, danger, className, children }: CardProps) {
  return (
    <section className={cx('card', danger && 'danger-zone', className)}>
      <h2>{title}</h2>
      {children}
    </section>
  );
}

interface SettingRowProps {
  title: ReactNode;
  /** Explanation under the title; an empty string keeps the line's space. */
  hint?: ReactNode;
  /** Separates the row from what comes before it with a rule. */
  divided?: boolean;
  /** Buttons shown on the right. */
  children?: ReactNode;
}

/** A setting's label and explanation on the left, its buttons on the right. */
export function SettingRow({ title, hint, divided, children }: SettingRowProps) {
  return (
    <div className={cx('row-between', divided && 'subtle-row')}>
      <div>
        <div>{title}</div>
        {hint != null && <div className="hint">{hint}</div>}
      </div>
      {children && <div className="btn-row">{children}</div>}
    </div>
  );
}
