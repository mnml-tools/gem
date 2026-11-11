import { describe, expect, it } from 'vitest';
import type { LogLine } from './ipc/bindings/LogLine';
import { LOG_CAPACITY, LogBuffer } from './logBuffer';

const lines = (from: number, count: number): LogLine[] =>
  Array.from({ length: count }, (_, index) => ({
    seq: from + index,
    ts: 1_700_000_000_000 + from + index,
    kind: 'status',
    text: `line ${from + index}`,
  }));

describe('LogBuffer', () => {
  it('keeps 5,000 lines per tab by default', () => {
    expect(new LogBuffer().capacity).toBe(LOG_CAPACITY);
    expect(LOG_CAPACITY).toBe(5000);
  });

  it('keeps lines in order until full', () => {
    const buffer = new LogBuffer(4);
    buffer.push(lines(1, 3));
    expect(buffer.size).toBe(3);
    expect(buffer.toArray().map((line) => line.seq)).toEqual([1, 2, 3]);
    expect(buffer.at(0)?.seq).toBe(1);
    expect(buffer.at(3)).toBeUndefined();
    expect(buffer.at(-1)).toBeUndefined();
  });

  it('drops the oldest lines when it overflows', () => {
    const buffer = new LogBuffer(4);
    buffer.push(lines(1, 3));
    buffer.push(lines(4, 3));
    expect(buffer.size).toBe(4);
    expect(buffer.toArray().map((line) => line.seq)).toEqual([3, 4, 5, 6]);
    expect(buffer.dropped).toBe(2);
  });

  it('keeps only the newest lines of a batch larger than the buffer', () => {
    const buffer = new LogBuffer(3);
    buffer.push(lines(1, 10));
    expect(buffer.toArray().map((line) => line.seq)).toEqual([8, 9, 10]);
    expect(buffer.dropped).toBe(7);
  });

  it('wraps around many times without losing order', () => {
    const buffer = new LogBuffer(5);
    for (let batch = 0; batch < 20; batch += 1) buffer.push(lines(batch * 3 + 1, 3));
    expect(buffer.toArray().map((line) => line.seq)).toEqual([56, 57, 58, 59, 60]);
  });

  it('bumps the version on each change, ignores empty batches, and clears', () => {
    const buffer = new LogBuffer(3);
    const start = buffer.version;
    buffer.push([]);
    expect(buffer.version).toBe(start);
    buffer.push(lines(1, 2));
    expect(buffer.version).toBe(start + 1);
    buffer.clear();
    expect(buffer.size).toBe(0);
    expect(buffer.dropped).toBe(0);
    expect(buffer.version).toBe(start + 2);
    buffer.push(lines(9, 1));
    expect(buffer.toArray()[0]?.seq).toBe(9);
  });

  it('refuses a zero capacity', () => {
    expect(() => new LogBuffer(0)).toThrow();
  });

  it('appends 5,000-line batches quickly', () => {
    const buffer = new LogBuffer();
    const batch = lines(0, 5000);
    const started = performance.now();
    for (let round = 0; round < 40; round += 1) buffer.push(batch);
    expect(performance.now() - started).toBeLessThan(500);
    expect(buffer.size).toBe(5000);
  });
});
