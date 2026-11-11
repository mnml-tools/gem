/**
 * The output of one remote command run (ADR 0014): stdout and stderr interleaved as they arrive,
 * with the stderr ranges remembered so the viewer can mark them. The text is bounded: past
 * `limit` characters the oldest whole lines are dropped (`dropped` counts them), so a
 * `tail -f` left running cannot grow without end. Offsets are absolute (they keep counting from
 * the first character ever received), so a viewer that synced up to an offset can catch up.
 */
import type { CommandStream } from './ipc/bindings/CommandStream';

/** About 5 MB of text per run. */
export const OUTPUT_LIMIT = 5 * 1024 * 1024;

export type Range = { from: number; to: number };

export class CommandOutput {
  readonly limit: number;
  /** Absolute offset of `text[0]`. */
  start = 0;
  text = '';
  /** Absolute stderr ranges, in order, within [start, end). */
  stderr: Range[] = [];
  /** Characters dropped from the front since the run started. */
  get dropped(): number {
    return this.start;
  }
  /** Absolute offset just past the last character. */
  get end(): number {
    return this.start + this.text.length;
  }
  version = 0;
  private listeners = new Set<() => void>();

  constructor(limit = OUTPUT_LIMIT) {
    this.limit = limit;
  }

  append(stream: CommandStream, chunk: string): void {
    if (!chunk) return;
    const from = this.end;
    this.text += chunk;
    if (stream === 'stderr') {
      const last = this.stderr[this.stderr.length - 1];
      if (last && last.to === from) last.to = this.end;
      else this.stderr.push({ from, to: this.end });
    }
    if (this.text.length > this.limit) this.trim();
    this.changed();
  }

  /** Called after every change (append, trim, clear); returns the unsubscribe function. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    this.version += 1;
    for (const listener of this.listeners) listener();
  }

  /** Drops the oldest lines so that about 90% of the limit remains. */
  private trim(): void {
    const excess = this.text.length - Math.floor(this.limit * 0.9);
    const newline = this.text.indexOf('\n', excess);
    const cut = newline >= 0 && newline < this.text.length - 1 ? newline + 1 : excess;
    this.text = this.text.slice(cut);
    this.start += cut;
    this.stderr = this.stderr
      .filter((range) => range.to > this.start)
      .map((range) => ({ from: Math.max(range.from, this.start), to: range.to }));
  }

  clear(): void {
    this.start = 0;
    this.text = '';
    this.stderr = [];
    this.changed();
  }

  /** Stderr ranges intersecting [from, to), clipped, relative to `base`. */
  stderrBetween(from: number, to: number, base: number): Range[] {
    const result: Range[] = [];
    for (const range of this.stderr) {
      if (range.to <= from || range.from >= to) continue;
      result.push({ from: Math.max(range.from, from) - base, to: Math.min(range.to, to) - base });
    }
    return result;
  }
}
