import { describe, expect, it } from 'vitest';
import { filterEntries, isDirLike, sortEntries } from './entries';
import type { Entry } from './ipc/bindings/Entry';
import { baseName, breadcrumbs, joinPath, nameProblem, normalizePath, parentPath } from './paths';

const entry = (name: string, patch: Partial<Entry> = {}): Entry => ({
  name,
  path: `/srv/${name}`,
  kind: 'file',
  size: 100,
  mtime: 1000,
  mode: 0o644,
  owner: 'deploy',
  group: 'deploy',
  linkTarget: null,
  targetKind: null,
  ...patch,
});

const entries = [
  entry('b.txt', { size: 300, mtime: 3000 }),
  entry('A.txt', { size: 100, mtime: 1000 }),
  entry('lib', { kind: 'dir', size: 4096, mtime: 5000 }),
  entry('file10.log', { size: 200, mtime: 2000 }),
  entry('file9.log', { size: null, mtime: null }),
  entry('current', { kind: 'symlink', targetKind: 'dir', linkTarget: '/srv/releases/1' }),
  entry('.env', { size: 50 }),
];

const names = (list: Entry[]) => list.map((item) => item.name);

describe('sortEntries', () => {
  it('puts folders (and links to folders) first, then names in natural order', () => {
    expect(names(sortEntries(entries, { key: 'name', descending: false }))).toEqual([
      'current',
      'lib',
      '.env',
      'A.txt',
      'b.txt',
      'file9.log',
      'file10.log',
    ]);
  });

  it('keeps folders first when descending', () => {
    expect(names(sortEntries(entries, { key: 'name', descending: true }))).toEqual([
      'lib',
      'current',
      'file10.log',
      'file9.log',
      'b.txt',
      'A.txt',
      '.env',
    ]);
  });

  it('sorts by size and date, unknown values first', () => {
    const bySize = sortEntries(entries, { key: 'size', descending: false });
    expect(names(bySize).slice(2)).toEqual(['file9.log', '.env', 'A.txt', 'file10.log', 'b.txt']);
    const byDate = sortEntries(entries, { key: 'modified', descending: true });
    expect(names(byDate).slice(2, 4)).toEqual(['b.txt', 'file10.log']);
  });

  it('does not mutate its input', () => {
    const copy = [...entries];
    sortEntries(entries, { key: 'size', descending: true });
    expect(entries).toEqual(copy);
  });

  it('knows what behaves like a folder', () => {
    expect(isDirLike(entry('x', { kind: 'dir' }))).toBe(true);
    expect(isDirLike(entry('x', { kind: 'symlink', targetKind: 'dir' }))).toBe(true);
    expect(isDirLike(entry('x', { kind: 'symlink', targetKind: 'file' }))).toBe(false);
    expect(isDirLike(entry('x', { kind: 'symlink', targetKind: null }))).toBe(false);
  });
});

describe('filterEntries', () => {
  it('hides dot files unless asked, and filters by substring, ignoring case', () => {
    expect(names(filterEntries(entries, false, ''))).not.toContain('.env');
    expect(names(filterEntries(entries, true, ''))).toContain('.env');
    expect(names(filterEntries(entries, false, 'FILE'))).toEqual(['file10.log', 'file9.log']);
    expect(names(filterEntries(entries, false, '  txt '))).toEqual(['b.txt', 'A.txt']);
  });
});

describe('paths', () => {
  it('normalises remote paths', () => {
    expect(normalizePath('/var//www/./api/../app/')).toBe('/var/www/app');
    expect(normalizePath('/..')).toBe('/');
    expect(normalizePath('a/../../b')).toBe('../b');
    expect(normalizePath('')).toBe('.');
  });

  it('joins, splits and walks up', () => {
    expect(joinPath('/var/www', 'api')).toBe('/var/www/api');
    expect(joinPath('/var/www', '/etc')).toBe('/etc');
    expect(joinPath('/', 'etc')).toBe('/etc');
    expect(parentPath('/var/www')).toBe('/var');
    expect(parentPath('/var')).toBe('/');
    expect(parentPath('/')).toBe('/');
    expect(baseName('/var/www/api/')).toBe('api');
    expect(baseName('/')).toBe('/');
  });

  it('builds breadcrumbs', () => {
    expect(breadcrumbs('/var/www')).toEqual([
      { name: '/', path: '/' },
      { name: 'var', path: '/var' },
      { name: 'www', path: '/var/www' },
    ]);
    expect(breadcrumbs('/')).toEqual([{ name: '/', path: '/' }]);
  });

  it('checks new names', () => {
    expect(nameProblem('')).toBe('empty');
    expect(nameProblem('  ')).toBe('empty');
    expect(nameProblem('a/b')).toBe('slash');
    expect(nameProblem('..')).toBe('dots');
    expect(nameProblem('config.yaml')).toBeNull();
  });
});
