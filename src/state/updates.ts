/**
 * In-app updates (ADR 0015): looks for a new version at startup (Settings › Check for updates
 * when Gem starts) or when asked in Settings, and keeps the update it found. The kit's
 * `UpdateDialog` downloads and installs it, then restarts Gem.
 */
import { useQuery } from '@tanstack/react-query';
import { isTauri } from '@tauri-apps/api/core';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { useEffect, useRef } from 'react';
import { create } from 'zustand';
import { errorMessage } from '@/lib/errors';
import { api } from '@/lib/ipc/api';
import { queryKeys } from '@/lib/query/keys';
import { updateStrings as t } from '@/strings/settings';

const CHECK_TIMEOUT_MS = 15_000;

type UpdatesState = {
  /** The newer version found by the last check, until it is dismissed. */
  update: Update | null;
  checking: boolean;
  /** Outcome of the last check asked for in Settings. */
  message: string | null;
  error: string | null;
  /**
   * Looks for a newer version; resolves to whether one was found. A `quiet` check (at startup)
   * only logs its failures.
   */
  check: (options?: { quiet?: boolean }) => Promise<boolean>;
  /** "Later": forgets the update until the next check. */
  dismiss: () => void;
};

const release = (update: Update | null) => void update?.close().catch(() => {});

export const useUpdates = create<UpdatesState>()((set, get) => ({
  update: null,
  checking: false,
  message: null,
  error: null,
  check: async ({ quiet = false } = {}) => {
    if (!isTauri() || get().checking) return false;
    set(quiet ? { checking: true } : { checking: true, message: null, error: null });
    try {
      const found = await check({ timeout: CHECK_TIMEOUT_MS });
      const previous = get().update;
      if (previous !== found) release(previous);
      set({ update: found });
      if (!quiet) set({ message: found ? t.available(found.version) : t.upToDate });
      return found != null;
    } catch (cause) {
      if (quiet) console.warn('Update check failed:', cause);
      else set({ error: t.failed(errorMessage(cause)) });
      return false;
    } finally {
      set({ checking: false });
    }
  },
  dismiss: () => {
    release(get().update);
    set({ update: null });
  },
}));

/** Checks once per launch when the setting is on; never in development. */
export function useStartupUpdateCheck(): void {
  const { data: settings } = useQuery({ queryKey: queryKeys.settings, queryFn: api.getSettings });
  const done = useRef(false);
  const enabled = settings?.checkUpdates;
  useEffect(() => {
    if (enabled === undefined || done.current) return;
    done.current = true;
    if (enabled && !import.meta.env.DEV) void useUpdates.getState().check({ quiet: true });
  }, [enabled]);
}
