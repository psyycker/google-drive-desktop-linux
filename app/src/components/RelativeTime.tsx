import { relativeTime } from '../lib/format';

/**
 * "5 min ago", with the full date as a tooltip. It is recomputed on each render;
 * the once-a-second status poll keeps it current.
 */
export function RelativeTime({ iso }: { iso: string }) {
  return (
    <time dateTime={iso} title={new Date(iso).toLocaleString()}>
      {relativeTime(iso)}
    </time>
  );
}
