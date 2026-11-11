/**
 * Loads a file tab's contents with `open_file`. Only the newest request per tab applies: a
 * preview replaced while loading, or a closed tab, ignores its late answer. A failed reload
 * of a document already on screen keeps it and returns the error for the caller to show.
 */
import type { QueryClient } from '@tanstack/react-query';
import { api } from '@/lib/ipc/api';
import { patchListingEntry } from '@/lib/query/listing';
import { toIpcError, type IpcError } from '@/lib/ipc/client';
import { useFileTabs } from '@/state/fileTabs';

const latestRequest = new Map<string, number>();
let counter = 0;

export async function loadFile(
  connId: string,
  sessionId: string,
  key: string,
  whole = false,
  queryClient?: QueryClient,
): Promise<{ ok: true } | { ok: false; error: IpcError | null }> {
  const id = `${connId}\n${key}`;
  counter += 1;
  const request = counter;
  latestRequest.set(id, request);
  const store = useFileTabs.getState();
  const tab = store.byConn[connId]?.tabs.find((item) => item.key === key);
  if (!tab) return { ok: false, error: null };
  store.update(connId, key, {
    busy: true,
    ...(tab.status === 'ready' ? {} : { status: 'loading' }),
  });
  try {
    const file = await api.openFile(sessionId, tab.path, whole);
    if (latestRequest.get(id) !== request) return { ok: false, error: null };
    useFileTabs.getState().loaded(connId, key, file, whole);
    if (queryClient) {
      patchListingEntry(queryClient, sessionId, file.path, { size: file.size, mtime: file.mtime });
    }
    return { ok: true };
  } catch (cause) {
    if (latestRequest.get(id) !== request) return { ok: false, error: null };
    const error = toIpcError(cause);
    const current = useFileTabs.getState().byConn[connId]?.tabs.find((item) => item.key === key);
    // A failed reload keeps the document that is already shown.
    useFileTabs.getState().update(connId, key, {
      busy: false,
      ...(current?.status === 'ready' ? {} : { status: 'error' }),
      error: { code: error.code, message: error.message },
    });
    return { ok: false, error: current?.status === 'ready' ? error : null };
  } finally {
    if (latestRequest.get(id) === request) latestRequest.delete(id);
  }
}
