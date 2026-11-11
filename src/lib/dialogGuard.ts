/**
 * Keeps modal dialogs from being answered by input that started before they opened.
 *
 * Gem opens dialogs from asynchronous results (a host-key prompt arrives some time after
 * "Connect"), so a dialog can appear in the middle of a pointer or key sequence: the release of
 * the click that started the connection, a held Enter that auto-repeats, the keyup of Space on
 * the button that just took the focus (Cancel, the autofocused default). Those must not choose
 * an answer the user never saw. The guard listens in the capture phase on the window, before
 * React, and swallows inside an open `<dialog>`:
 *
 * - a pointer click whose press started before the dialog opened;
 * - a keyboard click (Enter, Space, implicit form submission) whose key went down before;
 * - a `cancel` (Escape / close request) that no Escape pressed after the dialog opened caused.
 */

type Guard = { dispose: () => void };

let installed: (Guard & { users: number }) | null = null;

export function installDialogGuard(target: Window = window): () => void {
  if (installed) {
    installed.users += 1;
  } else {
    installed = { ...createGuard(target), users: 1 };
  }
  return () => {
    if (!installed) return;
    installed.users -= 1;
    if (installed.users === 0) {
      installed.dispose();
      installed = null;
    }
  };
}

function createGuard(target: Window): Guard {
  const document = target.document;
  /** A counter instead of timestamps: event order is all that matters. */
  let tick = 0;
  const openedAt = new WeakMap<Element, number>();
  let pointerDownAt = -1;
  /** The tick at which each held key first went down (auto-repeat keeps the first one). */
  const keysDownAt = new Map<string, number>();
  /** The key whose press is being activated right now (keydown Enter, keyup Space). */
  let activatingKeyAt = -1;
  let lastEscapeAt = -1;

  const markOpen = (dialog: Element) => {
    if (dialog instanceof HTMLDialogElement && dialog.open && !openedAt.has(dialog)) {
      openedAt.set(dialog, (tick += 1));
    } else if (dialog instanceof HTMLDialogElement && !dialog.open) {
      openedAt.delete(dialog);
    }
  };

  const observer = new MutationObserver((records) => {
    for (const record of records) {
      if (record.type === 'attributes' && record.target instanceof HTMLDialogElement) {
        markOpen(record.target);
      }
      for (const node of record.addedNodes) {
        if (node instanceof HTMLDialogElement) markOpen(node);
        if (node instanceof Element) node.querySelectorAll('dialog[open]').forEach(markOpen);
      }
    }
  });
  observer.observe(document.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ['open'],
  });
  document.querySelectorAll('dialog[open]').forEach(markOpen);

  const openDialogOf = (node: EventTarget | null): HTMLDialogElement | null => {
    const element = node instanceof Element ? node : null;
    const dialog = element?.closest('dialog');
    return dialog?.open ? dialog : null;
  };

  // A mutation record is delivered as a microtask: take any pending one into account first.
  const sync = () => {
    for (const record of observer.takeRecords()) {
      if (record.target instanceof HTMLDialogElement) markOpen(record.target);
      for (const node of record.addedNodes) {
        if (node instanceof HTMLDialogElement) markOpen(node);
        if (node instanceof Element) node.querySelectorAll('dialog[open]').forEach(markOpen);
      }
    }
  };

  const swallow = (event: Event) => {
    event.preventDefault();
    event.stopImmediatePropagation();
  };

  const onPointerDown = () => {
    sync();
    pointerDownAt = tick += 1;
  };

  const onKeyDown = (event: KeyboardEvent) => {
    sync();
    if (!event.repeat || !keysDownAt.has(event.key)) keysDownAt.set(event.key, (tick += 1));
    if (event.key === 'Escape') lastEscapeAt = keysDownAt.get('Escape') ?? -1;
    if (event.key === 'Enter') activate(keysDownAt.get('Enter') ?? -1);
    // A held Enter or Space that went down before a dialog opened does nothing inside it.
    const dialog = openDialogOf(event.target);
    const opened = dialog ? openedAt.get(dialog) : undefined;
    const pressedAt = keysDownAt.get(event.key) ?? -1;
    if (
      opened !== undefined &&
      pressedAt < opened &&
      (event.key === 'Enter' || event.key === ' ')
    ) {
      swallow(event);
    }
  };

  const onKeyUp = (event: KeyboardEvent) => {
    sync();
    const pressedAt = keysDownAt.get(event.key) ?? -1;
    keysDownAt.delete(event.key);
    if (event.key === ' ') activate(pressedAt);
    const dialog = openDialogOf(event.target);
    const opened = dialog ? openedAt.get(dialog) : undefined;
    if (opened !== undefined && pressedAt < opened && event.key === ' ') swallow(event);
  };

  let clearActivation: ReturnType<typeof setTimeout> | null = null;
  const activate = (pressedAt: number) => {
    activatingKeyAt = pressedAt;
    if (clearActivation) clearTimeout(clearActivation);
    // The click a key produces is dispatched in the same task as the key event.
    clearActivation = setTimeout(() => {
      activatingKeyAt = -1;
      clearActivation = null;
    }, 0);
  };

  const onClick = (event: MouseEvent) => {
    sync();
    const dialog = openDialogOf(event.target);
    const opened = dialog ? openedAt.get(dialog) : undefined;
    if (opened === undefined) return;
    if (event.detail > 0) {
      if (pointerDownAt < opened) swallow(event);
    } else if (activatingKeyAt >= 0 && activatingKeyAt < opened) {
      swallow(event);
    }
  };

  const onCancel = (event: Event) => {
    sync();
    const dialog = event.target instanceof HTMLDialogElement ? event.target : null;
    const opened = dialog ? openedAt.get(dialog) : undefined;
    if (opened === undefined) return;
    if (lastEscapeAt < opened) swallow(event);
  };

  target.addEventListener('pointerdown', onPointerDown, true);
  target.addEventListener('mousedown', onPointerDown, true);
  target.addEventListener('keydown', onKeyDown, true);
  target.addEventListener('keyup', onKeyUp, true);
  target.addEventListener('click', onClick, true);
  target.addEventListener('cancel', onCancel, true);
  const forget = () => keysDownAt.clear();
  target.addEventListener('blur', forget);

  return {
    dispose: () => {
      observer.disconnect();
      target.removeEventListener('pointerdown', onPointerDown, true);
      target.removeEventListener('mousedown', onPointerDown, true);
      target.removeEventListener('keydown', onKeyDown, true);
      target.removeEventListener('keyup', onKeyUp, true);
      target.removeEventListener('click', onClick, true);
      target.removeEventListener('cancel', onCancel, true);
      target.removeEventListener('blur', forget);
      if (clearActivation) clearTimeout(clearActivation);
    },
  };
}
