import { describe, expect, it } from 'vitest';
import {
  formatClock,
  formatDate,
  formatDateTime,
  formatEncoding,
  formatMode,
  formatOctal,
  formatSize,
  formatSpeed,
  parseOctal,
  parseRwx,
  plural,
} from './format';

describe('formatSize', () => {
  it.each([
    [0, '0 B'],
    [512, '512 B'],
    [1023, '1023 B'],
    [1024, '1.0 KB'],
    [2150, '2.1 KB'],
    [14 * 1024, '14 KB'],
    [2 * 1024 * 1024, '2.0 MB'],
    [48 * 1024 * 1024, '48 MB'],
    [200 * 1024 * 1024, '200 MB'],
    [1024 * 1024 - 1, '1.0 MB'],
    [1.5 * 1024 ** 3, '1.5 GB'],
    [3 * 1024 ** 4, '3.0 TB'],
  ])('%d → %s', (bytes, text) => {
    expect(formatSize(bytes)).toBe(text);
  });

  it('is empty for unknown sizes', () => {
    expect(formatSize(null)).toBe('');
    expect(formatSize(undefined)).toBe('');
    expect(formatSize(-1)).toBe('');
    expect(formatSize(Number.NaN)).toBe('');
  });

  it('formats speeds', () => {
    expect(formatSpeed(1.2 * 1024 * 1024)).toBe('1.2 MB/s');
    expect(formatSpeed(0)).toBe('0 B/s');
  });
});

describe('dates', () => {
  // Local time: the list shows the user's clock.
  const now = new Date(2026, 10, 8, 15, 30, 0);
  const at = (...parts: [number, number, number, number?, number?, number?]) =>
    new Date(...(parts as [number, number, number, number, number, number])).getTime() / 1000;

  it('shows the time today, "yesterday", the day this year, the full date before', () => {
    expect(formatDate(at(2026, 10, 8, 10, 42), now)).toBe('10:42');
    expect(formatDate(at(2026, 10, 7, 23, 59), now)).toBe('yesterday');
    expect(formatDate(at(2026, 2, 4, 9, 0), now)).toBe('04 Mar');
    expect(formatDate(at(2024, 2, 1, 9, 0), now)).toBe('2024-03-01');
    expect(formatDate(null, now)).toBe('');
  });

  it('shows future dates in full (clock skew between client and server)', () => {
    expect(formatDate(at(2026, 11, 24, 9, 0), now)).toBe('2026-12-24');
  });

  it('formats full timestamps and log clocks', () => {
    expect(formatDateTime(at(2026, 10, 7, 10, 42, 1))).toBe('2026-11-07 10:42:01');
    expect(formatDateTime(null)).toBe('');
    expect(formatClock(new Date(2026, 10, 7, 9, 5, 7).getTime())).toBe('09:05:07');
  });
});

describe('modes', () => {
  it.each([
    [0o755, 'dir', 'drwxr-xr-x'],
    [0o644, 'file', '-rw-r--r--'],
    [0o777, 'symlink', 'lrwxrwxrwx'],
    [0o600, 'file', '-rw-------'],
    [0o4755, 'file', '-rwsr-xr-x'],
    [0o4644, 'file', '-rwSr--r--'],
    [0o2775, 'dir', 'drwxrwsr-x'],
    [0o1777, 'dir', 'drwxrwxrwt'],
    [0o1776, 'dir', 'drwxrwxrwT'],
  ] as const)('%o (%s) → %s', (mode, kind, text) => {
    expect(formatMode(mode, kind)).toBe(text);
  });

  it('round-trips rwx text and bits', () => {
    for (const mode of [0o755, 0o644, 0o600, 0o4755, 0o2775, 0o1777, 0o1776, 0o4644, 0]) {
      expect(parseRwx(formatMode(mode, 'file'))).toBe(mode);
      expect(parseRwx(formatMode(mode, 'file').slice(1))).toBe(mode);
    }
    expect(parseRwx('rwxr-xr-')).toBeNull();
    expect(parseRwx('rwxr-xr-q')).toBeNull();
  });

  it('formats and parses octal', () => {
    expect(formatOctal(0o755)).toBe('755');
    expect(formatOctal(0o644)).toBe('644');
    expect(formatOctal(0o4755)).toBe('4755');
    expect(formatOctal(0o40755)).toBe('755');
    expect(formatOctal(0o7)).toBe('007');
    expect(parseOctal('755')).toBe(0o755);
    expect(parseOctal(' 0644 ')).toBe(0o644);
    expect(parseOctal('4755')).toBe(0o4755);
    expect(parseOctal('789')).toBeNull();
    expect(parseOctal('75')).toBeNull();
    expect(parseOctal('77777')).toBeNull();
    expect(formatMode(null)).toBe('');
  });
});

describe('labels', () => {
  it('formats encodings', () => {
    expect(formatEncoding('utf-8')).toBe('UTF-8');
    expect(formatEncoding('utf-8', true)).toBe('UTF-8 BOM');
    expect(formatEncoding('windows-1252')).toBe('Windows-1252');
    expect(formatEncoding('utf-16le', true)).toBe('UTF-16LE BOM');
  });

  it('uses real plurals', () => {
    expect(plural(1, 'item')).toBe('1 item');
    expect(plural(2, 'item')).toBe('2 items');
    expect(plural(10_000, 'entry', 'entries')).toBe('10,000 entries');
  });
});
