/** Shared queries for app-wide data (settings, sites, status). */
import { useQuery } from '@tanstack/react-query';
import { api } from '@/lib/ipc/api';
import type { Settings } from '@/lib/ipc/bindings/Settings';
import { queryKeys } from './keys';

/** Defaults used until `get_settings` answers (and when it fails). */
export const DEFAULT_SETTINGS: Settings = {
  downloadFolder: null,
  previewWindowKib: 2048,
  confirmAboveMib: 50,
  transferConcurrency: 2,
  keepaliveSecs: 30,
  showHidden: false,
  conflictPolicy: 'ask',
  checkUpdates: true,
};

export function useSettings(): Settings {
  const { data } = useQuery({ queryKey: queryKeys.settings, queryFn: api.getSettings });
  return data ?? DEFAULT_SETTINGS;
}

export function useSites() {
  return useQuery({ queryKey: queryKeys.sites, queryFn: api.listSites });
}

export function useAppStatus() {
  return useQuery({ queryKey: queryKeys.appStatus, queryFn: api.appStatus });
}

/** Global snippets, then those of `siteId` (ADR 0014); only fetched when `enabled`. */
export function useSnippets(siteId: string | null, enabled = true) {
  return useQuery({
    queryKey: queryKeys.snippets(siteId),
    queryFn: () => api.listSnippets(siteId),
    enabled,
  });
}
