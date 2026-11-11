/**
 * Uploads by OS drag and drop (plan §2.6). Rust owns the dropped paths: the window events
 * `gem://drag` (pointer position) and `gem://drop` (names and a drop id) only let the UI pick
 * the target folder — the folder row under the pointer, else the current folder — and call
 * `upload_dropped`. HTML5 drag and drop is never used for OS files.
 */
import { listen } from '@tauri-apps/api/event';
import { useEffect, useRef, useState } from 'react';
import { useToast } from '@/components/ui/Toast/context';
import { api, EVENTS } from '@/lib/ipc/api';
import type { DragHover } from '@/lib/ipc/bindings/DragHover';
import type { DropPayload } from '@/lib/ipc/bindings/DropPayload';
import { errorMessage } from '@/lib/errors';
import { useSessionData } from '@/state/sessionData';
import { connectionStrings as t } from '@/strings/connection';

type Options = {
  enabled: boolean;
  sessionId: string | null;
  connId: string;
  currentDir: string | null;
};

/** The folder a drop at (x, y) goes to: a folder row of the visible list, or `fallback`. */
export function dropTargetAt(x: number, y: number, fallback: string): string {
  const element =
    typeof document.elementFromPoint === 'function' ? document.elementFromPoint(x, y) : null;
  const row = element?.closest<HTMLElement>('[data-drop-dir]');
  return row?.dataset.dropDir ?? fallback;
}

/** Returns the highlighted target folder while files are dragged over the window. */
export function useOsDrop({ enabled, sessionId, connId, currentDir }: Options): string | null {
  const [target, setTarget] = useState<string | null>(null);
  const toast = useToast();
  const latest = useRef({ sessionId, currentDir, connId, toast });
  useEffect(() => {
    latest.current = { sessionId, currentDir, connId, toast };
  });

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    const stops: (() => void)[] = [];
    const keep = (stop: () => void) => (disposed ? stop() : stops.push(stop));

    listen<DragHover>(EVENTS.drag, ({ payload }) => {
      const dir = latest.current.currentDir;
      if (payload.kind === 'leave' || !dir) setTarget(null);
      else setTarget(dropTargetAt(payload.x, payload.y, dir));
    })
      .then(keep)
      .catch(() => {});

    listen<DropPayload>(EVENTS.drop, ({ payload }) => {
      setTarget(null);
      const { sessionId: id, currentDir: dir, connId: tab, toast: notify } = latest.current;
      if (!id || !dir) return;
      const remoteDir = dropTargetAt(payload.x, payload.y, dir);
      api
        .uploadDropped(id, payload.dropId, remoteDir)
        .then((transfers) => {
          useSessionData.getState().upsertTransfers(tab, transfers);
          if (transfers.length)
            notify.show({ message: t.transfersStarted(payload.names.length, false) });
        })
        .catch((cause: unknown) => notify.show({ message: errorMessage(cause), tone: 'error' }));
    })
      .then(keep)
      .catch(() => {});

    return () => {
      disposed = true;
      // Unlistening can fail when the window is going away; nothing is left to clean then.
      for (const stop of stops)
        Promise.resolve()
          .then(stop)
          .catch(() => {});
    };
  }, [enabled]);

  return enabled ? target : null;
}
