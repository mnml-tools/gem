/**
 * Fixed-capacity ring buffer for a connection's log (plan §2.7: 5,000 lines per tab). Appends
 * are O(1) and never allocate a new array; the oldest lines fall off the front. `version`
 * changes on every mutation so React can subscribe cheaply.
 */
import type { LogLine } from './ipc/bindings/LogLine';

export const LOG_CAPACITY = 5000;

export class LogBuffer {
  readonly capacity: number;
  private items: (LogLine | undefined)[];
  private start = 0;
  private count = 0;
  /** Lines dropped from the front since the buffer was created or cleared. */
  dropped = 0;
  version = 0;

  constructor(capacity = LOG_CAPACITY) {
    if (capacity < 1) throw new Error('capacity must be positive');
    this.capacity = capacity;
    this.items = new Array<LogLine | undefined>(capacity);
  }

  get size(): number {
    return this.count;
  }

  push(lines: readonly LogLine[]): void {
    if (!lines.length) return;
    // Only the newest `capacity` lines can survive.
    const from = Math.max(0, lines.length - this.capacity);
    this.dropped += from;
    for (let index = from; index < lines.length; index += 1) {
      const line = lines[index]!;
      if (this.count < this.capacity) {
        this.items[(this.start + this.count) % this.capacity] = line;
        this.count += 1;
      } else {
        this.items[this.start] = line;
        this.start = (this.start + 1) % this.capacity;
        this.dropped += 1;
      }
    }
    this.version += 1;
  }

  /** The line at `index`, 0 being the oldest kept line. */
  at(index: number): LogLine | undefined {
    if (index < 0 || index >= this.count) return undefined;
    return this.items[(this.start + index) % this.capacity];
  }

  toArray(): LogLine[] {
    const result: LogLine[] = [];
    for (let index = 0; index < this.count; index += 1) result.push(this.at(index)!);
    return result;
  }

  clear(): void {
    this.items = new Array<LogLine | undefined>(this.capacity);
    this.start = 0;
    this.count = 0;
    this.dropped = 0;
    this.version += 1;
  }
}
