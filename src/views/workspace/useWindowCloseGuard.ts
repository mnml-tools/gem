/**
 * Unsaved-changes protection for the window (Zap's pattern): `beforeunload` in a browser, and
 * Tauri's `onCloseRequested` in the app. `shouldBlock` returns `true` when it took over (it
 * shows a confirmation and calls `confirm` to close for real).
 */
import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useRef } from 'react';
import { pendingWork } from './closeTab';

export function useWindowCloseGuard(shouldBlock: (confirm: () => void) => boolean): void {
  const latest = useRef(shouldBlock);
  useEffect(() => {
    latest.current = shouldBlock;
  });

  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      const work = pendingWork();
      if (work.dirty || work.transfers) {
        event.preventDefault();
        event.returnValue = '';
      }
    };
    window.addEventListener('beforeunload', warn);

    let disposed = false;
    let confirmed = false;
    const subscription = isTauri()
      ? getCurrentWindow().onCloseRequested((event) => {
          if (confirmed) return;
          const blocked = latest.current(() => {
            confirmed = true;
            getCurrentWindow()
              .close()
              .catch((error: unknown) => {
                confirmed = false;
                console.error('Unable to close the window:', error);
              });
          });
          if (blocked) event.preventDefault();
        })
      : null;
    subscription
      ?.then((unlisten) => {
        if (disposed) unlisten();
      })
      .catch((error: unknown) => console.error('Unable to guard the window close:', error));

    return () => {
      disposed = true;
      window.removeEventListener('beforeunload', warn);
      subscription?.then((unlisten) => unlisten()).catch(() => {});
    };
  }, []);
}
