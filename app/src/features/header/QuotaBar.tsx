import type { Quota } from '../../api/types';
import { cx } from '../../lib/classNames';
import { formatBytes } from '../../lib/format';

export function QuotaBar({ quota: { used, limit } }: { quota: Quota }) {
  const pct = limit ? Math.min(100, (used / limit) * 100) : 0;
  return (
    <div className="quota">
      <div className="quota-bar">
        <div
          className={cx('quota-fill', pct >= 90 && pct < 100 && 'warn', pct >= 100 && 'full')}
          style={{ width: `${pct.toFixed(1)}%` }}
        />
      </div>
      <div className="quota-text">
        {limit
          ? `${formatBytes(used)} of ${formatBytes(limit)} used`
          : `${formatBytes(used)} used · unlimited storage`}
      </div>
    </div>
  );
}
