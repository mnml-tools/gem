/** Sorting and filtering of directory entries for the file list (plan §2.5). */
import type { Entry } from './ipc/bindings/Entry';
import { isHidden } from './paths';

export type SortKey = 'name' | 'size' | 'modified' | 'permissions' | 'owner';
export type SortOrder = { key: SortKey; descending: boolean };

/** Folders (and links to folders) sort before files, whatever the column. */
export function isDirLike(entry: Pick<Entry, 'kind' | 'targetKind'>): boolean {
  return entry.kind === 'dir' || (entry.kind === 'symlink' && entry.targetKind === 'dir');
}

const collator = new Intl.Collator('en', { numeric: true, sensitivity: 'base' });

function compareNullable(a: number | null, b: number | null): number {
  if (a == null && b == null) return 0;
  if (a == null) return -1;
  if (b == null) return 1;
  return a - b;
}

export function sortEntries(entries: readonly Entry[], order: SortOrder): Entry[] {
  const direction = order.descending ? -1 : 1;
  const byKey = (a: Entry, b: Entry): number => {
    switch (order.key) {
      case 'size':
        return compareNullable(a.size, b.size);
      case 'modified':
        return compareNullable(a.mtime, b.mtime);
      case 'permissions':
        return compareNullable(a.mode, b.mode);
      case 'owner':
        return collator.compare(a.owner ?? '', b.owner ?? '');
      case 'name':
        return 0;
    }
  };
  return [...entries].sort((a, b) => {
    const dirs = Number(isDirLike(b)) - Number(isDirLike(a));
    if (dirs) return dirs;
    const primary = byKey(a, b) * direction;
    if (primary) return primary;
    // Ties (and the name column) fall back to the name, in the chosen direction for names.
    return collator.compare(a.name, b.name) * (order.key === 'name' ? direction : 1);
  });
}

/** Hidden files (dot files) and the type-ahead filter (case-insensitive substring). */
export function filterEntries(entries: readonly Entry[], showHidden: boolean, filter: string) {
  const needle = filter.trim().toLowerCase();
  return entries.filter(
    (entry) =>
      (showHidden || !isHidden(entry.name)) &&
      (!needle || entry.name.toLowerCase().includes(needle)),
  );
}
