/**
 * A shortcut that wins over the focused widget: it listens in the capture phase on the window,
 * so it runs before CodeMirror (whose Mod+L selects a line) and before text fields. Used only for
 * keys that must work everywhere (Mod+W, Mod+L). Like the kit's registry, it stays quiet while a
 * modal dialog is open or an IME is composing.
 */
import { macPlatform, matchesShortcut, type Shortcut } from '@mnml-tools/ui';
import { useEffect, useRef } from 'react';

export function useWindowShortcut(shortcut: Shortcut, enabled: boolean, run: () => void): void {
  const latest = useRef({ enabled, run });
  useEffect(() => {
    latest.current = { enabled, run };
  });
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!latest.current.enabled || event.isComposing || event.repeat) return;
      if (!matchesShortcut(event, shortcut, macPlatform)) return;
      if (document.querySelector('dialog[open]')) return;
      event.preventDefault();
      event.stopPropagation();
      latest.current.run();
    };
    window.addEventListener('keydown', onKeyDown, true);
    return () => window.removeEventListener('keydown', onKeyDown, true);
  }, [shortcut]);
}
