import { describe, expect, it } from 'vitest';
import { CommandOutput } from './commandOutput';
import { fuzzyMatch } from './fuzzy';

describe('CommandOutput', () => {
  it('interleaves streams and merges adjacent stderr ranges', () => {
    const output = new CommandOutput();
    output.append('stdout', 'ok\n');
    output.append('stderr', 'bad\n');
    output.append('stderr', 'worse\n');
    output.append('stdout', 'fine\n');
    expect(output.text).toBe('ok\nbad\nworse\nfine\n');
    expect(output.stderr).toEqual([{ from: 3, to: 13 }]);
  });

  it('drops the oldest whole lines past its limit and keeps absolute offsets', () => {
    const output = new CommandOutput(100);
    for (let line = 0; line < 30; line += 1) {
      output.append(line === 25 ? 'stderr' : 'stdout', `line ${String(line).padStart(2, '0')}\n`);
    }
    expect(output.text.length).toBeLessThanOrEqual(100);
    expect(output.dropped).toBeGreaterThan(0);
    // Whole lines are dropped: the text starts at a line.
    expect(output.text.startsWith('line ')).toBe(true);
    expect(output.text.endsWith('line 29\n')).toBe(true);
    expect(output.end).toBe(30 * 8);
    const [range] = output.stderr;
    expect(output.text.slice(range!.from - output.start, range!.to - output.start)).toBe(
      'line 25\n',
    );
  });

  it('notifies subscribers and clears', () => {
    const output = new CommandOutput();
    let calls = 0;
    const stop = output.subscribe(() => (calls += 1));
    output.append('stdout', 'a');
    output.clear();
    stop();
    output.append('stdout', 'b');
    expect(calls).toBe(2);
    expect(output.text).toBe('b');
  });

  it('reports stderr ranges relative to a base', () => {
    const output = new CommandOutput();
    output.append('stdout', 'abc');
    output.append('stderr', 'def');
    expect(output.stderrBetween(4, 6, 2)).toEqual([{ from: 2, to: 4 }]);
  });
});

describe('fuzzyMatch', () => {
  it('ranks exact, prefix, word start, substring, then scattered', () => {
    expect(fuzzyMatch('uptime', 'uptime')?.rank).toBe(0);
    expect(fuzzyMatch('Core logs', 'core')?.rank).toBe(1);
    expect(fuzzyMatch('Core logs', 'logs')).toEqual({ rank: 2, indices: [5, 6, 7, 8] });
    expect(fuzzyMatch('crontab', 'tab')?.rank).toBe(3);
    expect(fuzzyMatch('Disk usage', 'dsk')).toEqual({ rank: 4, indices: [0, 2, 3] });
    expect(fuzzyMatch('Disk usage', 'xyz')).toBeNull();
    expect(fuzzyMatch('anything', '')).toEqual({ rank: 0, indices: [] });
  });
});
