/** Closing a connection tab: what would be lost, and the clean-up once confirmed. */
import type { QueryClient } from '@tanstack/react-query';
import { api } from '@/lib/ipc/api';
import { queryKeys } from '@/lib/query/keys';
import { dirtyFileTabs, useFileTabs } from '@/state/fileTabs';
import { FINISHED_STATES, useSessionData } from '@/state/sessionData';
import { getConnectionTab, useWorkspace } from '@/state/workspace';
import { forgetCommands } from '../commands/runner';

/** Unsaved files and running transfers of a tab (or of every tab). */
export function pendingWork(connId?: string): { dirty: number; transfers: number } {
  const transfers = useSessionData.getState().transfers;
  const groups = connId ? [transfers[connId] ?? []] : Object.values(transfers);
  return {
    dirty: dirtyFileTabs(connId).length,
    transfers: groups.flat().filter((item) => !FINISHED_STATES.has(item.state)).length,
  };
}

export function closeTabNow(id: string, queryClient: QueryClient): void {
  const tab = getConnectionTab(id);
  if (tab?.session) {
    void api.disconnect(tab.session.id).catch(() => {});
    queryClient.removeQueries({ queryKey: queryKeys.listings(tab.session.id) });
  }
  if (tab) {
    forgetCommands(id);
    useFileTabs.getState().closeAll(id);
    useSessionData.getState().forget(id);
  }
  useWorkspace.getState().removeTab(id);
}
