/**
 * The connection flow of one tab (plan §3.6): `connect` (or `reconnect` after a drop), and the
 * prompts its errors call for: a password or passphrase (`secret_required`), trusting a new
 * host key or certificate, and the blocking "identity changed" error. The session's channel
 * feeds the log, the state and the transfers of this tab.
 */
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '@/lib/ipc/api';
import type { HostKeyInfo } from '@/lib/ipc/bindings/HostKeyInfo';
import type { SessionEvent } from '@/lib/ipc/bindings/SessionEvent';
import { toIpcError } from '@/lib/ipc/client';
import { parentPath } from '@/lib/paths';
import { queryKeys } from '@/lib/query/keys';
import { takeSecret } from '@/state/secrets';
import { useSessionData } from '@/state/sessionData';
import { getConnectionTab, useWorkspace, type ConnectionTab } from '@/state/workspace';
import { commandStrings } from '@/strings/commands';
import { connectionStrings as t } from '@/strings/connection';
import { failRunningCommands } from '../commands/runner';

export type ConnectMode = 'connect' | 'reconnect';

export type SessionPrompt =
  | { kind: 'secret'; mode: ConnectMode }
  | { kind: 'trust'; mode: ConnectMode; key: HostKeyInfo; tls: boolean }
  | { kind: 'changed'; key: HostKeyInfo | null };

/** Whether the log of `connId` is on screen (errors there need no marker). */
function logVisible(connId: string): boolean {
  const { panelOpen, panelView } = useSessionData.getState();
  return panelOpen && panelView === 'log' && useWorkspace.getState().activeId === connId;
}

/** Applies one channel message to the stores. Exported for tests and the mock. */
export function handleSessionEvent(connId: string, event: SessionEvent, queryClient?: QueryClient) {
  const data = useSessionData.getState();
  switch (event.type) {
    case 'log':
      data.appendLog(connId, event.lines, logVisible(connId));
      return;
    case 'transfers': {
      const before = new Map((data.transfers[connId] ?? []).map((item) => [item.id, item.state]));
      data.upsertTransfers(connId, event.transfers);
      // A finished upload changes a listing: refresh the folders it went to.
      const tab = getConnectionTab(connId);
      const sessionId = tab?.session?.id;
      if (!sessionId || !queryClient) return;
      const dirs = new Set<string>();
      for (const transfer of event.transfers) {
        if (
          transfer.direction === 'upload' &&
          transfer.state === 'done' &&
          before.get(transfer.id) !== 'done'
        ) {
          dirs.add(parentPath(transfer.remotePath));
        }
      }
      for (const dir of dirs) {
        void queryClient.invalidateQueries({ queryKey: queryKeys.listing(sessionId, dir) });
      }
      return;
    }
    case 'state': {
      const tab = getConnectionTab(connId);
      if (!tab) return;
      const update = useWorkspace.getState().updateConnection;
      if (event.state === 'disconnected')
        failRunningCommands(connId, commandStrings.connectionLost);
      if (event.state === 'disconnected' && tab.status === 'connected') {
        update(connId, {
          status: 'disconnected',
          error: event.reason ? { code: 'disconnected', message: event.reason } : null,
        });
      } else if (event.state === 'connected' && tab.session && tab.status !== 'connected') {
        update(connId, { status: 'connected', error: null });
      }
      return;
    }
  }
}

export function useSession(tab: ConnectionTab) {
  const queryClient = useQueryClient();
  const [prompt, setPrompt] = useState<SessionPrompt | null>(null);
  const started = useRef(0);
  /** The secret of the current attempt, reused after trusting a host key (memory only). */
  const lastSecret = useRef<{ secret: string | null; remember: boolean }>({
    secret: null,
    remember: false,
  });
  const connId = tab.id;

  const run = useCallback(
    async (mode: ConnectMode, secret: string | null, remember: boolean) => {
      const current = getConnectionTab(connId);
      if (!current) return;
      const update = useWorkspace.getState().updateConnection;
      lastSecret.current = { secret, remember };
      try {
        const info =
          mode === 'reconnect' && current.session
            ? await api.reconnect(current.session.id, secret)
            : await api.connect(
                current.target,
                (event) => handleSessionEvent(connId, event, queryClient),
                secret,
                remember,
              );
        if (!getConnectionTab(connId)) {
          // The tab was closed while connecting.
          void api.disconnect(info.id).catch(() => {});
          return;
        }
        lastSecret.current = { secret: null, remember: false };
        const latest = getConnectionTab(connId);
        update(connId, {
          status: 'connected',
          error: null,
          session: info,
          path: latest?.path ?? info.startPath,
        });
        if (mode === 'reconnect') {
          void queryClient.invalidateQueries({ queryKey: queryKeys.listings(info.id) });
        }
      } catch (cause) {
        if (!getConnectionTab(connId)) return;
        const error = toIpcError(cause);
        switch (error.code) {
          case 'secret_required':
            setPrompt({ kind: 'secret', mode });
            return;
          case 'host_key_unknown':
          case 'certificate_untrusted':
            if (error.hostKey) {
              setPrompt({
                kind: 'trust',
                mode,
                key: error.hostKey,
                tls: error.code === 'certificate_untrusted',
              });
              return;
            }
            break;
          case 'host_key_changed':
            setPrompt({ kind: 'changed', key: error.hostKey });
            break;
        }
        update(connId, {
          status: mode === 'reconnect' ? 'disconnected' : 'failed',
          error: { code: error.code, message: error.message },
        });
      }
    },
    [connId, queryClient],
  );

  // A new attempt (opening the tab, Retry) starts a connection once, even under StrictMode.
  useEffect(() => {
    if (tab.status !== 'connecting' || tab.session || started.current === tab.attempt) return;
    started.current = tab.attempt;
    const held = takeSecret(connId);
    void run('connect', held?.secret ?? null, held?.remember ?? false);
  }, [tab.status, tab.session, tab.attempt, connId, run]);

  const reconnect = useCallback(() => {
    useWorkspace.getState().updateConnection(connId, { status: 'connecting', error: null });
    void run('reconnect', null, false);
  }, [connId, run]);

  const cancelPrompt = useCallback(() => {
    const current = prompt;
    setPrompt(null);
    if (current?.kind === 'changed') return;
    const mode = current?.mode ?? 'connect';
    useWorkspace.getState().updateConnection(connId, {
      status: mode === 'reconnect' ? 'disconnected' : 'failed',
      error: { code: 'cancelled', message: t.cancelled },
    });
  }, [connId, prompt]);

  const submitSecret = useCallback(
    (secret: string, remember: boolean) => {
      const mode = prompt?.kind === 'secret' ? prompt.mode : 'connect';
      setPrompt(null);
      void run(mode, secret, remember);
    },
    [prompt, run],
  );

  const trust = useCallback(async () => {
    if (prompt?.kind !== 'trust') return;
    const { key, mode } = prompt;
    setPrompt(null);
    try {
      await api.trustHostKey(key);
    } catch (cause) {
      const error = toIpcError(cause);
      useWorkspace.getState().updateConnection(connId, {
        status: mode === 'reconnect' ? 'disconnected' : 'failed',
        error: { code: error.code, message: error.message },
      });
      return;
    }
    await run(mode, lastSecret.current.secret, lastSecret.current.remember);
  }, [connId, prompt, run]);

  return { prompt, cancelPrompt, submitSecret, trust, reconnect };
}
