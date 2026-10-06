// Helpers for the Drive-relative paths ("Photos/2024/a.jpg") the daemon reports.

export const baseName = (p: string) => p.split('/').filter(Boolean).pop() ?? p;

export function parentOf(p: string): string {
  const parts = p.split('/').filter(Boolean);
  parts.pop();
  return parts.join('/');
}

/** "in Photos/2024", or "in My Drive" for top-level items. */
export function locationLabel(p: string): string {
  const parent = parentOf(p);
  return parent ? `in ${parent}` : 'in My Drive';
}

/** Joins a relative path onto the sync root; '' yields the root itself. */
export const joinRoot = (root: string, rel: string) => (rel ? `${root.replace(/\/+$/, '')}/${rel}` : root);
