/** Linux resize edges for the undecorated window (see `app/platform.ts`), after Piw's. */
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useEffect, useState } from 'react';
import { drawsResizeEdges } from '@/app/platform';

function useMaximized(): boolean {
  const [maximized, setMaximized] = useState(false);
  useEffect(() => {
    if (!drawsResizeEdges) return;
    const window = getCurrentWindow();
    let alive = true;
    const sync = () =>
      void window
        .isMaximized()
        .then((value) => {
          if (!alive) return;
          setMaximized(value);
          document.documentElement.toggleAttribute('data-maximized', value);
        })
        .catch(() => {});
    sync();
    const unlisten = window.onResized(sync);
    return () => {
      alive = false;
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, []);
  return maximized;
}

type Edge =
  'North' | 'South' | 'East' | 'West' | 'NorthEast' | 'NorthWest' | 'SouthEast' | 'SouthWest';
const EDGES: Edge[] = [
  'North',
  'South',
  'East',
  'West',
  'NorthEast',
  'NorthWest',
  'SouthEast',
  'SouthWest',
];

/** Invisible resize handles on the window edges (Linux, not maximized). */
export function ResizeEdges() {
  const maximized = useMaximized();
  if (!drawsResizeEdges || maximized) return null;
  const window = getCurrentWindow();
  return (
    <div aria-hidden className="gem-resize-edges">
      {EDGES.map((edge) => (
        // Pointer-only affordance (aria-hidden): keyboard users resize with the window manager,
        // exactly as with a native frame.
        // eslint-disable-next-line jsx-a11y-x/no-static-element-interactions
        <div
          key={edge}
          className="gem-resize-edge"
          data-edge={edge}
          onMouseDown={(event) => {
            if (event.button !== 0) return;
            event.preventDefault();
            void window.startResizeDragging(edge);
          }}
        />
      ))}
    </div>
  );
}
