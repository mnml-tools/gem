import { Dialog } from '@mnml-tools/ui';
import { act, fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useEffect, useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { installDialogGuard } from './dialogGuard';

let uninstall: () => void;
beforeEach(() => {
  uninstall = installDialogGuard();
});
afterEach(() => uninstall());

/** Opens its dialog `delay` ms after Connect, like a host-key prompt after `connect`. */
function Prompt({ onCancel, onTrust }: { onCancel: () => void; onTrust: () => void }) {
  const [asked, setAsked] = useState(false);
  const [open, setOpen] = useState(false);
  useEffect(() => {
    if (!asked) return;
    const timer = setTimeout(() => setOpen(true), 20);
    return () => clearTimeout(timer);
  }, [asked]);
  return (
    <>
      <button type="button" onClick={() => setAsked(true)}>
        Connect
      </button>
      {open && (
        <Dialog title="Trust this server?" role="alertdialog" onDismiss={onCancel}>
          <button type="button" data-autofocus onClick={onCancel}>
            Cancel
          </button>
          <button type="button" onClick={onTrust}>
            Trust and connect
          </button>
        </Dialog>
      )}
    </>
  );
}

const flush = () => act(() => new Promise((resolve) => setTimeout(resolve, 40)));

describe('dialog guard', () => {
  it('ignores the release of a press that started before the dialog opened', async () => {
    const onCancel = vi.fn();
    render(<Prompt onCancel={onCancel} onTrust={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }), { detail: 1 });
    // The pointer goes down for something else while the prompt is on its way…
    fireEvent.pointerDown(document.body);
    fireEvent.mouseDown(document.body);
    await flush();
    const cancel = await screen.findByRole('button', { name: 'Cancel' });
    // …and is released over the dialog once it is open.
    fireEvent.pointerUp(cancel);
    fireEvent.mouseUp(cancel);
    fireEvent.click(cancel, { detail: 1 });
    fireEvent.click(screen.getByRole('alertdialog'), { detail: 1 });
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByRole('alertdialog')).toHaveAttribute('open');
  });

  it('ignores a held key that went down before the dialog opened', async () => {
    const onCancel = vi.fn();
    render(<Prompt onCancel={onCancel} onTrust={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }), { detail: 1 });
    // Space and Enter go down while the prompt is on its way.
    fireEvent.keyDown(document.body, { key: ' ' });
    fireEvent.keyDown(document.body, { key: 'Enter' });
    await flush();
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toHaveFocus();
    // Space activates on release: the click it produces must not reach Cancel.
    fireEvent.keyUp(cancel, { key: ' ' });
    fireEvent.click(cancel, { detail: 0 });
    // The auto-repeat of the Enter held since then does nothing either.
    fireEvent.keyDown(cancel, { key: 'Enter', repeat: true });
    fireEvent.click(cancel, { detail: 0 });
    expect(onCancel).not.toHaveBeenCalled();
  });

  it('ignores a close request that no Escape caused, but honours Escape', async () => {
    const onCancel = vi.fn();
    render(<Prompt onCancel={onCancel} onTrust={() => {}} />);
    fireEvent.click(screen.getByRole('button', { name: 'Connect' }), { detail: 1 });
    await flush();
    const dialog = screen.getByRole('alertdialog');
    fireEvent(dialog, new Event('cancel', { cancelable: true }));
    expect(onCancel).not.toHaveBeenCalled();
    fireEvent.keyDown(dialog, { key: 'Escape' });
    fireEvent(dialog, new Event('cancel', { cancelable: true }));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('lets fresh clicks and keys through', async () => {
    const user = userEvent.setup();
    const onCancel = vi.fn();
    const onTrust = vi.fn();
    render(<Prompt onCancel={onCancel} onTrust={onTrust} />);
    await user.click(screen.getByRole('button', { name: 'Connect' }));
    await flush();
    await user.click(screen.getByRole('button', { name: 'Trust and connect' }));
    expect(onTrust).toHaveBeenCalledTimes(1);
    screen.getByRole('button', { name: 'Cancel' }).focus();
    await user.keyboard('{Enter}');
    expect(onCancel).toHaveBeenCalledTimes(1);
  });
});
