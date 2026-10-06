import { cx } from '../lib/classNames';
import './ProgressBar.css';

interface ProgressBarProps {
  /** Fraction done (0–1), or `null` when the total is unknown. */
  value: number | null;
  /** Downloads are green, everything else uses the accent colour. */
  tone?: 'accent' | 'download';
  className?: string;
}

export function ProgressBar({ value, tone = 'accent', className }: ProgressBarProps) {
  const pct = value === null ? 0 : Math.min(100, value * 100);
  return (
    <div className={cx('progress', tone === 'download' && 'down', value === null && 'indeterminate', className)}>
      <div style={{ width: `${pct.toFixed(1)}%` }} />
    </div>
  );
}
