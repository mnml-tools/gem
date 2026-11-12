import type { Update } from '@tauri-apps/plugin-updater';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useUpdates } from './updates';

const updater = vi.hoisted(() => ({ check: vi.fn(), tauri: true }));
vi.mock('@tauri-apps/plugin-updater', () => ({ check: updater.check }));
vi.mock('@tauri-apps/api/core', async (original) => ({
  ...(await original<object>()),
  isTauri: () => updater.tauri,
}));

const fakeUpdate = (version: string) =>
  ({ version, close: vi.fn(() => Promise.resolve()) }) as unknown as Update & {
    close: ReturnType<typeof vi.fn>;
  };

beforeEach(() => {
  updater.check.mockReset();
  updater.tauri = true;
  useUpdates.setState({ update: null, checking: false, message: null, error: null });
});

describe('update checks', () => {
  it('keeps the update it found and says so', async () => {
    const update = fakeUpdate('0.2.0');
    updater.check.mockResolvedValue(update);
    await expect(useUpdates.getState().check()).resolves.toBe(true);
    expect(useUpdates.getState()).toMatchObject({
      update,
      checking: false,
      message: 'Gem 0.2.0 is available.',
      error: null,
    });
  });

  it('says when Gem is up to date', async () => {
    updater.check.mockResolvedValue(null);
    await expect(useUpdates.getState().check()).resolves.toBe(false);
    expect(useUpdates.getState()).toMatchObject({ update: null, message: 'Gem is up to date.' });
  });

  it('shows the failure of a check asked for, and only logs it at startup', async () => {
    updater.check.mockRejectedValue('could not fetch a valid release JSON');
    await useUpdates.getState().check();
    expect(useUpdates.getState().error).toBe(
      'Could not check for updates: Could not fetch a valid release JSON',
    );

    useUpdates.setState({ error: null });
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await useUpdates.getState().check({ quiet: true });
    expect(useUpdates.getState()).toMatchObject({ error: null, message: null });
    expect(warn).toHaveBeenCalled();
  });

  it('releases an update that is replaced or dismissed', async () => {
    const first = fakeUpdate('0.2.0');
    const second = fakeUpdate('0.3.0');
    updater.check.mockResolvedValueOnce(first).mockResolvedValueOnce(second);
    await useUpdates.getState().check();
    await useUpdates.getState().check();
    expect(first.close).toHaveBeenCalled();
    useUpdates.getState().dismiss();
    expect(second.close).toHaveBeenCalled();
    expect(useUpdates.getState().update).toBeNull();
  });

  it('does nothing outside the desktop app', async () => {
    updater.tauri = false;
    await expect(useUpdates.getState().check()).resolves.toBe(false);
    expect(updater.check).not.toHaveBeenCalled();
  });
});
