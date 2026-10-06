/** "4.2 MB", "310 KB", "12 B" (binary units, like file managers). */
export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let v = n;
  let i = -1;
  do {
    v /= 1024;
    i++;
  } while (v >= 1024 && i < units.length - 1);
  const shown = v >= 100 || Math.abs(v - Math.round(v)) < 0.05 ? String(Math.round(v)) : v.toFixed(1);
  return `${shown} ${units[i]}`;
}

/** "just now", "5 min ago", "yesterday", "Mar 4". */
export function relativeTime(iso: string, now = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return '';
  const s = (now - t) / 1000;
  if (s < 45) return 'just now';
  if (s < 90) return '1 min ago';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d === 1) return 'yesterday';
  if (d < 7) return `${d} days ago`;
  return new Date(t).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

/** "1 item", "3 items". */
export const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "↓ 4.2 MB/s · ↑ 310 KB/s" for whichever directions are moving, or ''. */
export function speedText(downloadBps: number, uploadBps: number): string {
  const parts: string[] = [];
  if (downloadBps > 0) parts.push(`↓ ${formatBytes(downloadBps)}/s`);
  if (uploadBps > 0) parts.push(`↑ ${formatBytes(uploadBps)}/s`);
  return parts.join(' · ');
}

/** Compares "1.2.3"-style versions: negative, zero or positive. */
export function compareVersions(a: string, b: string): number {
  const parse = (v: string) =>
    v.replace(/^v/, '').split(/[-+]/)[0]!.split('.').map((n) => Number.parseInt(n, 10) || 0);
  const [x, y] = [parse(a), parse(b)];
  for (let i = 0; i < 3; i++) {
    const diff = (x[i] ?? 0) - (y[i] ?? 0);
    if (diff !== 0) return diff;
  }
  return 0;
}

/** A stable hue (0–359) derived from a string, e.g. for avatar colours. */
export function hueFor(s: string): number {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) % 360;
  return h;
}

/** Turns a rejected command or a thrown error into display text. */
export const errorMessage = (e: unknown): string => (e instanceof Error ? e.message : String(e));
