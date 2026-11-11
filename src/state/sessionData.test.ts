import { beforeEach, describe, expect, it } from 'vitest';
import type { Transfer } from '@/lib/ipc/bindings/Transfer';
import { logBuffer, transferSummary, useSessionData } from './sessionData';

const transfer = (id: string, patch: Partial<Transfer> = {}): Transfer => ({
  id,
  sessionId: 's1',
  direction: 'upload',
  name: `${id}.bin`,
  localPath: `/home/you/${id}.bin`,
  remotePath: `/srv/${id}.bin`,
  size: 1000,
  done: 0,
  state: 'queued',
  error: null,
  startedAt: null,
  finishedAt: null,
  ...patch,
});

beforeEach(() => {
  useSessionData.getState().forget('t1');
  useSessionData.setState({ transfers: {}, speeds: {}, unseenError: {}, logVersion: {} });
});

describe('transfers', () => {
  it('merges changed transfers by id and keeps the first-seen order', () => {
    const data = useSessionData.getState();
    data.upsertTransfers('t1', [transfer('a'), transfer('b')]);
    data.upsertTransfers('t1', [transfer('b', { state: 'running', done: 10 }), transfer('c')]);
    const list = useSessionData.getState().transfers['t1']!;
    expect(list.map((item) => `${item.id}:${item.state}`)).toEqual([
      'a:queued',
      'b:running',
      'c:queued',
    ]);
  });

  it('measures speeds of running transfers and sums them for the status bar', () => {
    const data = useSessionData.getState();
    data.upsertTransfers('t1', [transfer('a', { state: 'running', done: 0 })], 1000);
    data.upsertTransfers('t1', [transfer('a', { state: 'running', done: 500 })], 2000);
    expect(useSessionData.getState().speeds['a']).toBe(500);
    data.upsertTransfers('t1', [transfer('b', { state: 'queued', direction: 'download' })], 2000);
    const summary = transferSummary(
      useSessionData.getState().transfers['t1']!,
      useSessionData.getState().speeds,
    );
    expect(summary).toEqual({ uploads: 1, downloads: 1, rate: 500 });
    data.upsertTransfers('t1', [transfer('a', { state: 'done', done: 1000 })], 3000);
    expect(useSessionData.getState().speeds['a']).toBeUndefined();
  });

  it('clears finished transfers only', () => {
    const data = useSessionData.getState();
    data.upsertTransfers('t1', [
      transfer('a', { state: 'done' }),
      transfer('b', { state: 'failed' }),
      transfer('c', { state: 'running' }),
      transfer('d', { state: 'conflict' }),
    ]);
    data.removeFinished('t1');
    expect(useSessionData.getState().transfers['t1']!.map((item) => item.id)).toEqual(['c', 'd']);
  });
});

describe('log', () => {
  it('marks errors that arrive while the log is not visible', () => {
    const data = useSessionData.getState();
    data.appendLog('t1', [{ seq: 1, ts: 1, kind: 'status', text: 'ok' }], false);
    expect(useSessionData.getState().unseenError['t1']).toBeFalsy();
    data.appendLog('t1', [{ seq: 2, ts: 2, kind: 'error', text: 'boom' }], true);
    expect(useSessionData.getState().unseenError['t1']).toBeFalsy();
    data.appendLog('t1', [{ seq: 3, ts: 3, kind: 'error', text: 'boom' }], false);
    expect(useSessionData.getState().unseenError['t1']).toBe(true);
    data.markLogSeen('t1');
    expect(useSessionData.getState().unseenError['t1']).toBe(false);
    expect(logBuffer('t1').size).toBe(3);
  });

  it('forgets everything of a closed tab', () => {
    const data = useSessionData.getState();
    data.appendLog('t1', [{ seq: 1, ts: 1, kind: 'error', text: 'x' }], false);
    data.upsertTransfers('t1', [transfer('a')]);
    data.forget('t1');
    const state = useSessionData.getState();
    expect(state.transfers['t1']).toBeUndefined();
    expect(state.unseenError['t1']).toBeUndefined();
    expect(logBuffer('t1').size).toBe(0);
  });
});
