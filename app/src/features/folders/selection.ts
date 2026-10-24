// Folder selection as Drive-relative paths (`Work/Reports`), the way the daemon
// takes it. `null` stands for the whole Drive. A selected folder includes everything
// inside it, so a selection never lists a folder together with one inside it.

/** Whether `path` is `folder` or inside it. */
export const within = (path: string, folder: string) => path === folder || path.startsWith(`${folder}/`);

export interface FolderCheck {
  checked: boolean;
  /** Some folder inside it is selected, but not all of it. */
  partly: boolean;
  /** The selected folder it is part of, when that is a parent of it. */
  includedWith: string | null;
}

export function checkOf(selection: string[], path: string): FolderCheck {
  const includedWith = selection.find((f) => f !== path && within(path, f)) ?? null;
  const checked = includedWith !== null || selection.includes(path);
  const partly = !checked && selection.some((f) => f !== path && within(f, path));
  return { checked, partly, includedWith };
}

/** Selects `path` (in place of anything selected inside it), or unselects it. */
export function toggle(selection: string[], path: string): string[] {
  if (selection.includes(path)) return selection.filter((f) => f !== path);
  return [...selection.filter((f) => !within(f, path)), path].sort();
}

export function sameSelection(a: string[] | null, b: string[] | null): boolean {
  if (a === null || b === null) return a === b;
  const [x, y] = [[...a].sort(), [...b].sort()];
  return x.length === y.length && x.every((f, i) => f === y[i]);
}

export interface SelectionChange {
  /** Newly synced folders. */
  added: string[];
  /** Folders that stop syncing, so their local copies go. */
  removed: string[];
  /** Everything outside the new selection stops syncing (it was the whole Drive). */
  narrowsWholeDrive: boolean;
}

export function changeOf(saved: string[] | null, next: string[] | null): SelectionChange {
  const covered = (sel: string[] | null, path: string) => sel === null || sel.some((f) => within(path, f));
  return {
    added: (next ?? []).filter((f) => !covered(saved, f)),
    removed: (saved ?? []).filter((f) => !covered(next, f)),
    narrowsWholeDrive: saved === null && next !== null,
  };
}
