/**
 * Formatting of remote metadata for the file list, the viewer footer, transfers and the status
 * bar. Everything is pure and locale-independent (the UI is English, mono columns must align).
 */
import type { EntryKind } from './ipc/bindings/EntryKind';
import type { LineEnding } from './ipc/bindings/LineEnding';

const UNITS = ['KB', 'MB', 'GB', 'TB'] as const;

/** `512 B`, `2.1 KB`, `14 KB`, `2.0 MB`, `48 MB` (binary multiples, as file managers show). */
export function formatSize(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // One decimal below 10 (2.1 KB), none above (14 KB); never "1024 KB".
  let text = value < 10 ? value.toFixed(1) : String(Math.round(value));
  if (text === '1024' && unit < UNITS.length - 1) {
    text = '1.0';
    unit += 1;
  }
  return `${text} ${UNITS[unit]}`;
}

/** Bytes per second as `1.2 MB/s`. */
export function formatSpeed(bytesPerSecond: number): string {
  if (!Number.isFinite(bytesPerSecond) || bytesPerSecond <= 0) return '0 B/s';
  return `${formatSize(bytesPerSecond)}/s`;
}

const pad = (value: number) => String(value).padStart(2, '0');
const MONTHS = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
] as const;

function sameDay(a: Date, b: Date): boolean {
  return (
    a.getFullYear() === b.getFullYear() &&
    a.getMonth() === b.getMonth() &&
    a.getDate() === b.getDate()
  );
}

/**
 * Short modification date for the list: `10:42` today, `yesterday`, `07 Nov` this year,
 * `2024-03-01` before. `mtime` is in seconds since the Unix epoch.
 */
export function formatDate(mtime: number | null | undefined, now: Date = new Date()): string {
  if (mtime == null) return '';
  const date = new Date(mtime * 1000);
  if (sameDay(date, now)) return `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  if (sameDay(date, yesterday)) return 'yesterday';
  if (date.getFullYear() === now.getFullYear() && date <= now) {
    return `${pad(date.getDate())} ${MONTHS[date.getMonth()]}`;
  }
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** Full local timestamp for tooltips: `2026-11-07 10:42:01`. */
export function formatDateTime(mtime: number | null | undefined): string {
  if (mtime == null) return '';
  const d = new Date(mtime * 1000);
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

/** Log timestamps: `10:42:01` (milliseconds since the epoch). */
export function formatClock(ms: number): string {
  const d = new Date(ms);
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

const KIND_CHAR: Record<EntryKind, string> = { dir: 'd', symlink: 'l', file: '-', other: '-' };

/** `drwxr-xr-x`, with setuid/setgid/sticky as `s`/`S`/`t`/`T` like `ls -l`. */
export function formatMode(mode: number | null | undefined, kind: EntryKind = 'file'): string {
  if (mode == null) return '';
  const triplet = (shift: number, special: boolean, specialChar: string) => {
    const r = mode & (4 << shift) ? 'r' : '-';
    const w = mode & (2 << shift) ? 'w' : '-';
    const executable = (mode & (1 << shift)) !== 0;
    const x = special
      ? executable
        ? specialChar
        : specialChar.toUpperCase()
      : executable
        ? 'x'
        : '-';
    return r + w + x;
  };
  return (
    KIND_CHAR[kind] +
    triplet(6, (mode & 0o4000) !== 0, 's') +
    triplet(3, (mode & 0o2000) !== 0, 's') +
    triplet(0, (mode & 0o1000) !== 0, 't')
  );
}

/** `755`, or `4755` when a special bit is set. */
export function formatOctal(mode: number): string {
  const masked = mode & 0o7777;
  const text = masked.toString(8);
  return masked > 0o777 ? text.padStart(4, '0') : text.padStart(3, '0');
}

/** Parses `755` / `0755` / `4755`; `null` when it is not a valid mode. */
export function parseOctal(text: string): number | null {
  const trimmed = text.trim();
  if (!/^[0-7]{3,4}$/.test(trimmed)) return null;
  return parseInt(trimmed, 8);
}

/** Parses `rwxr-xr-x` (9 characters, optionally prefixed by the kind) into permission bits. */
export function parseRwx(text: string): number | null {
  const value = text.length === 10 ? text.slice(1) : text;
  if (!/^([r-][w-][xsStT-]){3}$/.test(value)) return null;
  let mode = 0;
  const specials = [0o4000, 0o2000, 0o1000];
  for (let group = 0; group < 3; group += 1) {
    const shift = 6 - group * 3;
    const [r, w, x] = value.slice(group * 3, group * 3 + 3);
    if (r === 'r') mode |= 4 << shift;
    if (w === 'w') mode |= 2 << shift;
    if (x === 'x' || x === 's' || x === 't') mode |= 1 << shift;
    if (x === 's' || x === 'S' || x === 't' || x === 'T') mode |= specials[group] ?? 0;
  }
  return mode;
}

export const LINE_ENDING_LABEL: Record<LineEnding, string> = { lf: 'LF', crlf: 'CRLF', cr: 'CR' };

/** `UTF-8`, `Windows-1252`, `UTF-16LE` (+ ` BOM`). */
export function formatEncoding(encoding: string, bom = false): string {
  const label = encoding.toLowerCase();
  const name = label.startsWith('windows-') ? `Windows-${label.slice(8)}` : label.toUpperCase();
  return bom ? `${name} BOM` : name;
}

/** `1 item`, `2 items`. */
export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count.toLocaleString('en-US')} ${count === 1 ? singular : pluralForm}`;
}
