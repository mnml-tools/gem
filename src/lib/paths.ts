/** POSIX path helpers for remote paths (the UI never sees local paths). */

/** Collapses `//`, resolves `.` and `..`, keeps a leading `/`, drops a trailing one. */
export function normalizePath(path: string): string {
  const absolute = path.startsWith('/');
  const parts: string[] = [];
  for (const part of path.split('/')) {
    if (!part || part === '.') continue;
    if (part === '..') {
      if (parts.length && parts[parts.length - 1] !== '..') parts.pop();
      else if (!absolute) parts.push('..');
      continue;
    }
    parts.push(part);
  }
  const joined = parts.join('/');
  return absolute ? `/${joined}` : joined || '.';
}

export function joinPath(dir: string, name: string): string {
  if (name.startsWith('/')) return normalizePath(name);
  return normalizePath(`${dir}/${name}`);
}

/** The parent directory; `/` for `/` and for top-level entries. */
export function parentPath(path: string): string {
  const normalized = normalizePath(path);
  if (normalized === '/') return '/';
  const index = normalized.lastIndexOf('/');
  return index <= 0 ? '/' : normalized.slice(0, index);
}

export function baseName(path: string): string {
  const normalized = normalizePath(path);
  if (normalized === '/') return '/';
  return normalized.slice(normalized.lastIndexOf('/') + 1);
}

export type Crumb = { name: string; path: string };

/** `/var/www` → `/`, `var`, `www` with their paths. */
export function breadcrumbs(path: string): Crumb[] {
  const normalized = normalizePath(path);
  const crumbs: Crumb[] = [{ name: '/', path: '/' }];
  if (normalized === '/') return crumbs;
  let current = '';
  for (const part of normalized.split('/').filter(Boolean)) {
    current += `/${part}`;
    crumbs.push({ name: part, path: current });
  }
  return crumbs;
}

/** Why a new file or folder name cannot be used, or `null` when it is fine. */
export function nameProblem(name: string): 'empty' | 'slash' | 'dots' | null {
  if (!name.trim()) return 'empty';
  if (name.includes('/') || name.includes('\0')) return 'slash';
  if (name === '.' || name === '..') return 'dots';
  return null;
}

export const isHidden = (name: string) => name.startsWith('.');
