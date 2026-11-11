/**
 * What sessions push through their channel (plan §2.7, §2.6): the log ring buffer and the
 * transfer queue of each connection tab, plus the bottom panel's layout. Log buffers are kept
 * outside React state (they mutate in place); `logVersion` tells subscribers to re-read.
 */
import { create } from 'zustand';
import type { LogLine } from '@/lib/ipc/bindings/LogLine';
import type { Transfer } from '@/lib/ipc/bindings/Transfer';
import { LogBuffer } from '@/lib/logBuffer';

export type PanelView = 'log' | 'transfers';

const PANEL_OPEN_KEY = 'gem.panelOpen';
const PANEL_VIEW_KEY = 'gem.panelView';

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* Keep the session usable without storage. */
  }
}

const buffers = new Map<string, LogBuffer>();

export function logBuffer(connId: string): LogBuffer {
  let buffer = buffers.get(connId);
  if (!buffer) {
    buffer = new LogBuffer();
    buffers.set(connId, buffer);
  }
  return buffer;
}

/** Speed samples per transfer, for the rate shown in rows and in the status bar. */
type Sample = { done: number; at: number; speed: number };
const samples = new Map<string, Sample>();

type SessionDataState = {
  panelOpen: boolean;
  panelView: PanelView;
  logVersion: Record<string, number>;
  /** An error line arrived while the log of that tab was not visible. */
  unseenError: Record<string, boolean>;
  transfers: Record<string, Transfer[]>;
  speeds: Record<string, number>;
  setPanelOpen: (open: boolean) => void;
  togglePanel: () => void;
  setPanelView: (view: PanelView) => void;
  appendLog: (connId: string, lines: LogLine[], visible: boolean) => void;
  clearLog: (connId: string) => void;
  markLogSeen: (connId: string) => void;
  upsertTransfers: (connId: string, transfers: Transfer[], now?: number) => void;
  removeFinished: (connId: string) => void;
  forget: (connId: string) => void;
};

export const FINISHED_STATES = new Set<Transfer['state']>([
  'done',
  'failed',
  'cancelled',
  'skipped',
]);

export const useSessionData = create<SessionDataState>()((set) => ({
  panelOpen: read(PANEL_OPEN_KEY) !== 'false',
  panelView: read(PANEL_VIEW_KEY) === 'transfers' ? 'transfers' : 'log',
  logVersion: {},
  unseenError: {},
  transfers: {},
  speeds: {},

  setPanelOpen: (open) => {
    write(PANEL_OPEN_KEY, String(open));
    set({ panelOpen: open });
  },
  togglePanel: () =>
    set((state) => {
      write(PANEL_OPEN_KEY, String(!state.panelOpen));
      return { panelOpen: !state.panelOpen };
    }),
  setPanelView: (view) => {
    write(PANEL_VIEW_KEY, view);
    set({ panelView: view });
  },

  appendLog: (connId, lines, visible) => {
    const buffer = logBuffer(connId);
    buffer.push(lines);
    const error = !visible && lines.some((line) => line.kind === 'error');
    set((state) => ({
      logVersion: { ...state.logVersion, [connId]: buffer.version },
      unseenError: error ? { ...state.unseenError, [connId]: true } : state.unseenError,
    }));
  },
  clearLog: (connId) => {
    const buffer = logBuffer(connId);
    buffer.clear();
    set((state) => ({ logVersion: { ...state.logVersion, [connId]: buffer.version } }));
  },
  markLogSeen: (connId) =>
    set((state) =>
      state.unseenError[connId]
        ? { unseenError: { ...state.unseenError, [connId]: false } }
        : state,
    ),

  upsertTransfers: (connId, incoming, now = Date.now()) =>
    set((state) => {
      const current = state.transfers[connId] ?? [];
      const byId = new Map(current.map((item) => [item.id, item]));
      const order = current.map((item) => item.id);
      const speeds = { ...state.speeds };
      for (const transfer of incoming) {
        if (!byId.has(transfer.id)) order.push(transfer.id);
        byId.set(transfer.id, transfer);
        if (transfer.state === 'running') {
          const previous = samples.get(transfer.id);
          if (previous && now > previous.at && transfer.done >= previous.done) {
            const instant = ((transfer.done - previous.done) * 1000) / (now - previous.at);
            // Smooth the rate so the number does not flicker.
            const speed = previous.speed ? previous.speed * 0.6 + instant * 0.4 : instant;
            samples.set(transfer.id, { done: transfer.done, at: now, speed });
            speeds[transfer.id] = speed;
          } else if (!previous) {
            samples.set(transfer.id, { done: transfer.done, at: now, speed: 0 });
          }
        } else {
          samples.delete(transfer.id);
          delete speeds[transfer.id];
        }
      }
      return {
        transfers: { ...state.transfers, [connId]: order.map((id) => byId.get(id)!) },
        speeds,
      };
    }),
  removeFinished: (connId) =>
    set((state) => ({
      transfers: {
        ...state.transfers,
        [connId]: (state.transfers[connId] ?? []).filter(
          (item) => !FINISHED_STATES.has(item.state),
        ),
      },
    })),

  forget: (connId) => {
    buffers.delete(connId);
    set((state) => {
      for (const transfer of state.transfers[connId] ?? []) samples.delete(transfer.id);
      const without = <T>(record: Record<string, T>) => {
        const copy = { ...record };
        delete copy[connId];
        return copy;
      };
      return {
        logVersion: without(state.logVersion),
        unseenError: without(state.unseenError),
        transfers: without(state.transfers),
      };
    });
  },
}));

const EMPTY_TRANSFERS: Transfer[] = [];

export function useTransfers(connId: string | null): Transfer[] {
  return useSessionData((state) => (connId ? state.transfers[connId] : null) ?? EMPTY_TRANSFERS);
}

/** Running transfer counts and the total rate, for the status bar. */
export function transferSummary(transfers: Transfer[], speeds: Record<string, number>) {
  let uploads = 0;
  let downloads = 0;
  let rate = 0;
  for (const transfer of transfers) {
    if (transfer.state !== 'running' && transfer.state !== 'queued') continue;
    if (transfer.direction === 'upload') uploads += 1;
    else downloads += 1;
    rate += speeds[transfer.id] ?? 0;
  }
  return { uploads, downloads, rate };
}
