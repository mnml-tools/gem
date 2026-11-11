/**
 * The fake remote server of the browser preview: one POSIX-like tree shared by every demo
 * site. Text files hold their logical content (`\n` lines); how they are stored on the
 * "server" (line endings, encoding) is described next to it, so sizes and the viewer footer
 * look real. Synthetic data only (plan §9.1).
 */
import type { EntryKind } from '@/lib/ipc/bindings/EntryKind';
import type { LineEnding } from '@/lib/ipc/bindings/LineEnding';
import { baseName, joinPath, normalizePath, parentPath } from '@/lib/paths';
import { logLine, SAMPLE_FILES } from './mock-samples';

export type FileNode = {
  kind: 'file';
  text: string;
  /** How the bytes are stored on the server. */
  lineEnding: LineEnding;
  mixed: boolean;
  encoding: string;
  binary: boolean;
  /** Reported size in bytes (generated files are never materialised). */
  size: number;
  generated: 'biglog' | null;
  mode: number;
  mtime: number;
  owner: string;
  group: string;
};

export type DirNode = {
  kind: 'dir';
  mode: number;
  mtime: number;
  owner: string;
  group: string;
  /** Reading the folder fails with `permission_denied`. */
  denied: boolean;
  /** Saving inside it cannot use a temp file + rename (`unsupported` until allowed). */
  noTempFiles: boolean;
};

export type LinkNode = {
  kind: 'symlink';
  target: string;
  mtime: number;
  owner: string;
  group: string;
};

export type Node = FileNode | DirNode | LinkNode;

export const BIG_LOG_SIZE = 200 * 1024 * 1024;
export const MANY_FILES = 10_000;

const NOW = Math.floor(Date.now() / 1000);
const DAY = 86_400;

/** Deterministic pseudo-random numbers, so the demo looks the same on every reload. */
function hash(value: number): number {
  let h = value | 0;
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b);
  h = Math.imul(h ^ (h >>> 16), 0x45d9f3b);
  return (h ^ (h >>> 16)) >>> 0;
}

const encoder = new TextEncoder();

/** Byte size of a text as stored with its encoding and line endings. */
export function storedSize(text: string, encoding: string, lineEnding: LineEnding, mixed: boolean) {
  const bytes = encoding === 'windows-1252' ? text.length : encoder.encode(text).length;
  const lines = text.split('\n').length - 1;
  if (mixed) return bytes + Math.floor(lines / 2);
  return lineEnding === 'crlf' ? bytes + lines : bytes;
}

export class MockTree {
  readonly nodes = new Map<string, Node>();

  constructor(seed = true) {
    this.dir('/', { owner: 'root' });
    if (seed) this.seed();
  }

  get(path: string): Node | undefined {
    return this.nodes.get(normalizePath(path));
  }

  /** Follows symlinks (a few levels) to the final node. */
  resolve(path: string): { path: string; node: Node | undefined } {
    let current = normalizePath(path);
    for (let depth = 0; depth < 8; depth += 1) {
      const node = this.nodes.get(current);
      if (node?.kind !== 'symlink') return { path: current, node };
      current = joinPath(parentPath(current), node.target);
    }
    return { path: current, node: undefined };
  }

  children(dir: string): string[] {
    const prefix = dir === '/' ? '/' : `${dir}/`;
    const result: string[] = [];
    for (const path of this.nodes.keys()) {
      if (path !== dir && path.startsWith(prefix) && !path.slice(prefix.length).includes('/')) {
        result.push(path);
      }
    }
    return result;
  }

  dir(path: string, options: Partial<DirNode> = {}): void {
    const normalized = normalizePath(path);
    if (normalized !== '/' && !this.nodes.has(parentPath(normalized)))
      this.dir(parentPath(normalized));
    const existing = this.nodes.get(normalized);
    this.nodes.set(normalized, {
      kind: 'dir',
      mode: 0o755,
      mtime: NOW - 3 * DAY,
      owner: 'deploy',
      group: 'www-data',
      denied: false,
      noTempFiles: false,
      ...(existing?.kind === 'dir' ? existing : {}),
      ...options,
    });
  }

  file(path: string, text: string, options: Partial<FileNode> = {}): FileNode {
    const normalized = normalizePath(path);
    this.dir(parentPath(normalized));
    const encoding = options.encoding ?? 'utf-8';
    const lineEnding = options.lineEnding ?? 'lf';
    const mixed = options.mixed ?? false;
    const node: FileNode = {
      kind: 'file',
      text,
      lineEnding,
      mixed,
      encoding,
      binary: false,
      size: storedSize(text, encoding, lineEnding, mixed),
      generated: null,
      mode: 0o644,
      mtime: NOW - (hash(normalized.length * 31 + text.length) % (40 * DAY)),
      owner: 'deploy',
      group: 'www-data',
      ...options,
    };
    this.nodes.set(normalized, node);
    return node;
  }

  link(path: string, target: string): void {
    const normalized = normalizePath(path);
    this.dir(parentPath(normalized));
    this.nodes.set(normalized, {
      kind: 'symlink',
      target,
      mtime: NOW - 2 * DAY,
      owner: 'deploy',
      group: 'deploy',
    });
  }

  remove(path: string): number {
    const normalized = normalizePath(path);
    let count = 0;
    for (const key of [...this.nodes.keys()]) {
      if (key === normalized || key.startsWith(`${normalized}/`)) {
        this.nodes.delete(key);
        count += 1;
      }
    }
    return count;
  }

  move(from: string, to: string): void {
    const source = normalizePath(from);
    const target = normalizePath(to);
    for (const [key, node] of [...this.nodes.entries()]) {
      if (key === source || key.startsWith(`${source}/`)) {
        this.nodes.delete(key);
        this.nodes.set(target + key.slice(source.length), node);
      }
    }
  }

  kindOf(node: Node | undefined): EntryKind | null {
    return node ? node.kind : null;
  }

  private seed(): void {
    for (const [path, text] of Object.entries(SAMPLE_FILES)) {
      if (path.includes('/legacy/')) continue;
      const mode = text.startsWith('#!') ? 0o755 : 0o644;
      this.file(path, text, { mode });
    }

    // Encodings and line endings (plan §2.3 seed list).
    this.file(
      '/var/www/api/legacy/latin1.txt',
      SAMPLE_FILES['/var/www/api/legacy/latin1.txt'] ?? '',
      {
        encoding: 'windows-1252',
      },
    );
    this.file(
      '/var/www/api/legacy/windows.ini',
      SAMPLE_FILES['/var/www/api/legacy/windows.ini'] ?? '',
      {
        lineEnding: 'crlf',
      },
    );
    this.file(
      '/var/www/api/legacy/mixed.txt',
      SAMPLE_FILES['/var/www/api/legacy/mixed.txt'] ?? '',
      {
        lineEnding: 'lf',
        mixed: true,
      },
    );

    // The 200 MB log served in windows, and a medium one that loads whole.
    this.file('/var/www/api/storage/logs/app.log', '', {
      generated: 'biglog',
      size: BIG_LOG_SIZE,
      mtime: NOW - 40,
      owner: 'www-data',
    });
    const access: string[] = [];
    for (let index = 0; index < 60_000; index += 1) access.push(logLine(index * 7 + 3));
    this.file('/var/www/api/storage/logs/access.log', `${access.join('\n')}\n`, {
      mtime: NOW - 600,
      owner: 'www-data',
    });

    // Binary files.
    this.file('/var/www/api/public/assets/logo.png', '', { binary: true, size: 48_213 });
    this.file('/var/www/api/storage/backup-2026-11-01.tar.gz', '', {
      binary: true,
      size: 734_003_200,
      mtime: NOW - 7 * DAY,
    });

    // Symlinks: to a folder, to a file, and a broken one.
    this.dir('/var/www/releases/2026-11-07');
    this.file('/var/www/releases/2026-11-07/VERSION', '2026.11.07-3\n');
    this.dir('/var/www/releases/2026-10-28');
    this.link('/var/www/api/current', '/var/www/releases/2026-11-07');
    this.link('/var/www/api/latest.log', 'storage/logs/laravel.log');
    this.link('/var/www/api/old-config.yaml', 'config.old.yaml');
    this.link('/etc/nginx/sites-enabled/default', '../sites-available/default');

    // A folder the user may not read, and one that refuses temp files on save.
    this.dir('/var/www/api/private', { denied: true, owner: 'root', group: 'root', mode: 0o700 });
    this.dir('/root', { denied: true, owner: 'root', group: 'root', mode: 0o700 });
    this.dir('/etc/nginx', { owner: 'root', group: 'root', noTempFiles: true });
    this.dir('/etc/nginx/sites-available', { owner: 'root', group: 'root', noTempFiles: true });

    // Empty folders and some structure around.
    this.dir('/var/www/api/storage/cache');
    this.dir('/var/www/api/vendor/laravel/framework');
    this.file(
      '/var/www/api/vendor/autoload.php',
      "<?php\n\nrequire __DIR__ . '/composer/autoload_real.php';\n",
    );
    this.dir('/home/deploy');
    this.dir('/tmp', { mode: 0o1777, owner: 'root', group: 'root' });
    this.dir('/srv/tools/target');

    // A folder with 10,000 entries (plan §3.3: must stay fluid).
    this.dir('/var/cache/thumbs', { owner: 'www-data' });
    for (let index = 0; index < MANY_FILES; index += 1) {
      const h = hash(index + 1);
      const name = `thumb-${String(index).padStart(5, '0')}-${(h % 0xfffff).toString(16)}.webp`;
      this.nodes.set(`/var/cache/thumbs/${name}`, {
        kind: 'file',
        text: '',
        lineEnding: 'lf',
        mixed: false,
        encoding: 'utf-8',
        binary: true,
        size: 2_000 + (h % 90_000),
        generated: null,
        mode: 0o644,
        mtime: NOW - (h % (300 * DAY)),
        owner: 'www-data',
        group: 'www-data',
      });
    }
  }
}

/**
 * Reads the generated 200 MB log by byte window. Lines are generated on demand and their
 * offsets remembered, so sequential "Load more" stays cheap.
 */
export class BigLog {
  private offsets: number[] = [0];

  private ensure(offset: number): void {
    let last = this.offsets[this.offsets.length - 1]!;
    while (last <= offset && last < BIG_LOG_SIZE) {
      const index = this.offsets.length - 1;
      last += logLine(index).length + 1;
      this.offsets.push(last);
    }
  }

  private lineAt(offset: number): number {
    this.ensure(offset);
    let low = 0;
    let high = this.offsets.length - 1;
    while (low < high) {
      const middle = (low + high + 1) >> 1;
      if (this.offsets[middle]! <= offset) low = middle;
      else high = middle - 1;
    }
    return low;
  }

  /** Text of [offset, offset + length), clipped to the file. */
  read(offset: number, length: number): string {
    const end = Math.min(BIG_LOG_SIZE, offset + length);
    if (offset >= end) return '';
    let line = this.lineAt(offset);
    const parts: string[] = [];
    let position = this.offsets[line]!;
    while (position < end) {
      const text = `${logLine(line)}\n`;
      const from = Math.max(0, offset - position);
      const to = Math.min(text.length, end - position);
      parts.push(text.slice(from, to));
      position += text.length;
      line += 1;
      if (this.offsets.length <= line) this.offsets.push(position);
    }
    return parts.join('');
  }
}

export const nameOf = baseName;
