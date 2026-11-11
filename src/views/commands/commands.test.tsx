import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { getFileTabs } from '@/state/fileTabs';
import { useWorkspace } from '@/state/workspace';
import { cleanupApp, renderApp } from '@/test/render';

afterEach(() => cleanupApp());

type User = ReturnType<typeof userEvent.setup>;

async function connectProd(user: User) {
  await user.click(await screen.findByRole('button', { name: 'Connect to prod-api' }));
  const dialog = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
  await user.click(within(dialog).getByRole('button', { name: 'Trust and connect' }));
  await screen.findByRole('listbox', { name: 'Files in /var/www/api' });
}

const connId = () => useWorkspace.getState().activeId!;
const commandTabs = () => getFileTabs(connId()).tabs.filter((tab) => tab.command);

async function openPalette(user: User) {
  await user.keyboard('{Control>}k{/Control}');
  return screen.findByRole('dialog', { name: 'Run a command' });
}

const bar = () => document.querySelector('.gem-command-bar') as HTMLElement;

async function snippetMenuItem(user: User, name: RegExp) {
  await user.click(screen.getByRole('button', { name: 'Snippets' }));
  return screen.findByRole('menuitem', { name });
}

describe('command palette', () => {
  it('filters snippets with their scope, offers the free text last, and runs it', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const palette = await openPalette(user);
    const input = within(palette).getByRole('combobox', { name: 'Run a command' });
    expect(input).toHaveFocus();

    await user.type(input, 'core');
    const options = within(palette).getAllByRole('option');
    expect(options[0]).toHaveTextContent(/^Core logs/);
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    expect(options.at(-1)).toHaveTextContent('Run core');
    expect(options.at(-1)).toHaveAttribute('aria-selected', 'false');
    expect(within(palette).getByRole('option', { name: /Core logs.*Global/ })).toBeInTheDocument();
    expect(within(palette).getByRole('option', { name: /Restart core.*Site/ })).toBeInTheDocument();

    await user.clear(input);
    await user.type(input, 'echo hello{Enter}');
    expect(screen.queryByRole('dialog', { name: 'Run a command' })).toBeNull();
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    expect(bar()).toHaveTextContent('echo hello');
    expect(commandTabs()).toHaveLength(1);
  });

  it('Enter reuses a finished tab of the same command; Mod+Enter opens another', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const run = async (modifier: boolean) => {
      const palette = await openPalette(user);
      await user.type(within(palette).getByRole('combobox'), 'echo up');
      await user.keyboard(modifier ? '{Control>}{Enter}{/Control}' : '{Enter}');
      await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    };
    await run(false);
    await run(false);
    expect(commandTabs()).toHaveLength(1);
    await run(true);
    expect(commandTabs()).toHaveLength(2);
  });

  it('runs the best match on Enter, not the partial text typed', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'Upt{Enter}');
    expect(await screen.findByRole('button', { name: 'Uptime' })).toBeInTheDocument();
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    expect(bar()).toHaveTextContent('uptime');
  });

  it('offers only "Run …", selected, when nothing matches', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'zzqx');
    const options = within(palette).getAllByRole('option');
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent('Run zzqx');
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
  });

  it('selects a recent command equal to the text, without a duplicate "Run …"', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    let palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'echo Uptime{Enter}');
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'echo Uptime');
    const options = within(palette).getAllByRole('option');
    expect(options[0]).toHaveTextContent(/^echo Uptime.*Recent/);
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    expect(within(palette).queryByRole('option', { name: 'Run echo Uptime' })).toBeNull();
  });

  it('keeps recent free commands in memory only', async () => {
    const user = userEvent.setup();
    const setItem = vi.spyOn(Storage.prototype, 'setItem');
    renderApp();
    await connectProd(user);
    let palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'echo s3cr3t-token{Enter}');
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    palette = await openPalette(user);
    expect(
      within(palette).getByRole('option', { name: /echo s3cr3t-token.*Recent/ }),
    ).toBeInTheDocument();
    await user.keyboard('{Escape}');
    const written = setItem.mock.calls.map((call) => call.join('='));
    expect(written.some((text) => text.includes('s3cr3t'))).toBe(false);
  });

  it('is not offered on FTP: no Snippets button, and Mod+K explains why', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Connect to ftp-legacy' }));
    await screen.findByRole('listbox', { name: /^Files in/ });
    expect(screen.queryByRole('button', { name: 'Snippets' })).toBeNull();
    const palette = await openPalette(user);
    expect(palette).toHaveTextContent('Commands need an SSH (SFTP) connection.');
    expect(within(palette).queryByRole('combobox')).toBeNull();
  });
});

describe('command output tab', () => {
  it('streams output, marks stderr, and ends with the exit status', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.click(await snippetMenuItem(user, /^Core logs/));
    expect(await screen.findByRole('button', { name: 'Core logs' })).toBeInTheDocument();
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    expect(bar()).toHaveTextContent('docker logs core | tail -30');
    const output = await screen.findByRole('textbox', { name: 'Output of Core logs' });
    await waitFor(() => expect(output).toHaveTextContent('ERROR [queue] job BuildExport'));
    const marked = [...document.querySelectorAll('.cm-gem-stderr')].map((node) => node.textContent);
    expect(marked.join('')).toContain('SQLSTATE[HY000]');
    expect(marked.join('')).not.toContain('[http]');
    // The footer's language picker starts on Log.
    expect(screen.getByRole('combobox', { name: 'Language' })).toHaveValue('log');
  });

  it('shows a failing command in the danger state', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'frobnicate --all{Enter}');
    await waitFor(() => expect(bar()).toHaveTextContent('Exit 127'));
    expect(bar().querySelector('[data-state="error"]')).not.toBeNull();
  });

  it('stops a long-running command, re-runs it in the same tab, and asks before closing it', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.click(await snippetMenuItem(user, /^Follow app log/));
    await waitFor(() => expect(bar()).toHaveTextContent(/Running/));
    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: 'Output of Follow app log' })).toHaveTextContent(
        'production.INFO',
      ),
    );
    await user.click(within(bar()).getByRole('button', { name: 'Stop' }));
    await waitFor(() => expect(bar()).toHaveTextContent(/Stopped/));

    await user.click(within(bar()).getByRole('button', { name: 'Re-run' }));
    await waitFor(() => expect(bar()).toHaveTextContent(/Running/));
    expect(commandTabs()).toHaveLength(1);

    // Closing a running command asks first.
    await user.click(screen.getByRole('button', { name: 'Close Follow app log' }));
    const ask = await screen.findByRole('alertdialog', { name: 'Stop the command?' });
    await user.click(within(ask).getByRole('button', { name: 'Stop and close' }));
    await waitFor(() => expect(commandTabs()).toHaveLength(0));
  });

  it('fails running commands when the connection drops; Re-run waits for a reconnect', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    await connectProd(user);
    await user.click(await snippetMenuItem(user, /^Follow app log/));
    await waitFor(() => expect(bar()).toHaveTextContent(/Running/));
    controls.dropConnection();
    await waitFor(() => expect(bar()).toHaveTextContent(/Failed/));
    expect(within(bar()).getByRole('button', { name: 'Re-run' })).toBeDisabled();
  });
});

describe('after a run', () => {
  it('refreshes the folder it ran in, keeping the selection and the focus', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    const files = screen.getByRole('listbox', { name: 'Files in /var/www/api' });
    await user.click(within(files).getByRole('option', { name: /^composer\.json/ }));
    const palette = await openPalette(user);
    await user.type(within(palette).getByRole('combobox'), 'touch made-by-touch.txt{Enter}');
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    const focused = document.activeElement;
    expect(
      await within(files).findByRole('option', { name: /^made-by-touch\.txt/ }),
    ).toBeInTheDocument();
    expect(within(files).getByRole('option', { name: /^composer\.json/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(document.activeElement).toBe(focused);

    // A removal shows too.
    const again = await openPalette(user);
    await user.type(within(again).getByRole('combobox'), 'rm made-by-touch.txt{Enter}');
    await waitFor(() =>
      expect(within(files).queryByRole('option', { name: /^made-by-touch\.txt/ })).toBeNull(),
    );
  });
});

describe('confirm before running', () => {
  it('shows the command as it will run, and runs it only when confirmed', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.click(await snippetMenuItem(user, /^Restart core/));
    let ask = await screen.findByRole('alertdialog', { name: 'Run this command?' });
    expect(ask).toHaveTextContent('docker restart core');
    await user.click(within(ask).getByRole('button', { name: 'Cancel' }));
    expect(commandTabs()).toHaveLength(0);

    await user.click(await snippetMenuItem(user, /^Restart core/));
    ask = await screen.findByRole('alertdialog', { name: 'Run this command?' });
    await user.click(within(ask).getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    expect(commandTabs()[0]?.name).toBe('Restart core');
  });

  it('previews {dir} as the current folder, quoted', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.click(await snippetMenuItem(user, /^Manage snippets/));
    const dialog = await screen.findByRole('dialog', { name: 'Snippets' });
    const site = within(dialog).getByRole('region', { name: 'This site (prod-api)' });
    await user.click(within(site).getByRole('button', { name: 'Add snippet' }));
    const editor = await screen.findByRole('dialog', { name: 'New snippet' });
    await user.type(within(editor).getByLabelText(/^Name/), 'List here');
    await user.type(within(editor).getByLabelText(/^Command/), 'ls -la {{dir}');
    await user.click(within(editor).getByRole('checkbox', { name: 'Ask before running' }));
    await user.click(within(editor).getByRole('button', { name: 'Save snippet' }));
    await user.click(await screen.findByRole('button', { name: 'Done' }));
    await user.click(await snippetMenuItem(user, /^List here/));
    const ask = await screen.findByRole('alertdialog', { name: 'Run this command?' });
    expect(ask).toHaveTextContent("ls -la '/var/www/api'");
    await user.click(within(ask).getByRole('button', { name: 'Run' }));
    await waitFor(() => expect(bar()).toHaveTextContent(/Exit 0/));
    expect(bar()).toHaveTextContent("ls -la '/var/www/api'");
  });
});

describe('manage snippets', () => {
  async function openManage(user: User) {
    await user.click(await snippetMenuItem(user, /^Manage snippets/));
    return screen.findByRole('dialog', { name: 'Snippets' });
  }

  it('validates, creates, moves between scopes, reorders and deletes', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await openManage(user);
    const global = () =>
      within(screen.getByRole('dialog', { name: 'Snippets' })).getByRole('region', {
        name: 'Global',
      });
    const site = () =>
      within(screen.getByRole('dialog', { name: 'Snippets' })).getByRole('region', {
        name: 'This site (prod-api)',
      });

    // Validation.
    await user.click(within(global()).getByRole('button', { name: 'Add snippet' }));
    let editor = await screen.findByRole('dialog', { name: 'New snippet' });
    await user.click(within(editor).getByRole('button', { name: 'Save snippet' }));
    expect(within(editor).getByText('Enter a name')).toBeInTheDocument();
    expect(within(editor).getByText('Enter a command')).toBeInTheDocument();

    // Create a global snippet.
    await user.type(within(editor).getByLabelText(/^Name/), 'Memory');
    await user.type(within(editor).getByLabelText(/^Command/), 'free -m');
    await user.click(within(editor).getByRole('button', { name: 'Save snippet' }));
    await screen.findByRole('dialog', { name: 'Snippets' });
    expect(within(global()).getByRole('listitem', { name: 'Memory' })).toBeInTheDocument();

    // Move it to this site.
    await user.click(within(global()).getByRole('button', { name: 'Edit Memory' }));
    editor = await screen.findByRole('dialog', { name: 'Edit snippet' });
    await user.click(within(editor).getByRole('button', { name: 'This site' }));
    await user.click(within(editor).getByRole('button', { name: 'Save snippet' }));
    await waitFor(() =>
      expect(within(site()).getByRole('listitem', { name: 'Memory' })).toBeInTheDocument(),
    );
    expect(within(global()).queryByRole('listitem', { name: 'Memory' })).toBeNull();

    // Reorder with the keyboard (Alt+↑) and with the buttons.
    const names = () =>
      within(site())
        .getAllByRole('listitem')
        .map((item) => item.getAttribute('aria-label'));
    expect(names()).toEqual(['Restart core', 'Follow app log', 'Memory']);
    within(site()).getByRole('listitem', { name: 'Memory' }).focus();
    await user.keyboard('{Alt>}{ArrowUp}{/Alt}');
    await waitFor(() => expect(names()).toEqual(['Restart core', 'Memory', 'Follow app log']));
    await user.click(within(site()).getByRole('button', { name: 'Move Memory up' }));
    await waitFor(() => expect(names()).toEqual(['Memory', 'Restart core', 'Follow app log']));

    // Delete, after a confirmation.
    await user.click(within(site()).getByRole('button', { name: 'Delete Memory' }));
    const ask = await screen.findByRole('alertdialog', { name: 'Delete snippet?' });
    await user.click(within(ask).getByRole('button', { name: 'Delete snippet' }));
    await waitFor(() => expect(names()).toEqual(['Restart core', 'Follow app log']));
    // The list stays open after the deletion.
    expect(screen.getByRole('dialog', { name: 'Snippets' })).toBeInTheDocument();
  });
});
