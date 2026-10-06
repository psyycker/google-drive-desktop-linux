import type { ReactNode } from 'react';

import { cx } from '../../lib/classNames';

interface StepProps {
  number: number;
  title: string;
  /** `disabled` greys the step out and hides its body. */
  state: 'active' | 'done' | 'disabled';
  /** Shown instead of the body, e.g. once the step is done. */
  summary?: string;
  /** Shown at the right of the heading, e.g. an Edit link. */
  action?: ReactNode;
  children: ReactNode;
}

export function Step({ number, title, state, summary, action, children }: StepProps) {
  return (
    <div className={cx('step', state === 'done' && 'done', state === 'disabled' && 'disabled')}>
      <div className="step-head">
        <span className="step-num">{number}</span>
        <div className="step-title">{title}</div>
        {action}
      </div>
      {state !== 'disabled' &&
        (summary ? <div className="step-done">{summary}</div> : <div className="step-body">{children}</div>)}
    </div>
  );
}
