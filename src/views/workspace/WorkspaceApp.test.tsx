import { EditorView } from '@codemirror/view';
import { fireEvent, screen, waitFor, within } from '@testing-library/react';
import type { Update } from '@tauri-apps/plugin-updater';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { MockControls } from '@/dev/mock-backend';
import { api } from '@/lib/ipc/api';
import { useSessionData } from '@/state/sessionData';
import { useUpdates } from '@/state/updates';
import { cleanupApp, renderApp } from '@/test/render';

afterEach(() => cleanupApp());

type User = ReturnType<typeof userEvent.setup>;

/** Connects to a demo site from the Sites page and answers the host-key prompt. */
async function connectProd(user: User) {
  await user.click(await screen.findByRole('button', { name: 'Connect to prod-api' }));
  const dialog = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
  await user.click(within(dialog).getByRole('button', { name: 'Trust and connect' }));
  return screen.findByRole('listbox', { name: 'Files in /var/www/api' });
}

const row = (name: string) => screen.findByRole('option', { name });

/** The CodeMirror view showing a file, once it has loaded. */
async function editorFor(name: string): Promise<EditorView> {
  const content = await screen.findByRole('textbox', { name }, { timeout: 3000 });
  const view = EditorView.findFromDOM(content);
  if (!view) throw new Error('no editor');
  return view;
}

describe('connection flow', () => {
  it('asks to trust an unknown host key, then lists the start folder (plan §3.6)', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    await user.click(await screen.findByRole('button', { name: 'Connect to prod-api' }));

    const dialog = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
    expect(within(dialog).getByText('prod.example.com:22')).toBeInTheDocument();
    expect(within(dialog).getByText('ssh-ed25519')).toBeInTheDocument();
    expect(within(dialog).getByText(/^SHA256:pV6m2Ug0/)).toBeInTheDocument();
    expect(controls.knownHosts.has('prod.example.com:22')).toBe(false);

    await user.click(within(dialog).getByRole('button', { name: 'Trust and connect' }));
    const list = await screen.findByRole('listbox', { name: 'Files in /var/www/api' });
    expect(await within(list).findByRole('option', { name: 'config.yaml' })).toBeInTheDocument();
    expect(controls.knownHosts.has('prod.example.com:22')).toBe(true);

    // The tab, the title bar and the status bar follow the session.
    expect(screen.getAllByRole('img', { name: 'Connected' })).toHaveLength(2);
    expect(screen.getByRole('contentinfo')).toHaveTextContent(/^GEM \/ prod-api·\/var\/www\/api/);
    // The log shows the conversation.
    expect(await screen.findByText('opendir /var/www/api')).toBeInTheDocument();
  });

  it('cancelling the host key prompt leaves the tab failed with Retry', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Connect to prod-api' }));
    const dialog = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(await screen.findByRole('heading', { name: 'Couldn’t connect' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Retry' })).toBeInTheDocument();
  });

  it('asks for a password, and shows a failed login in the log in the danger style (CA-07)', async () => {
    const user = userEvent.setup();
    const { container } = renderApp();
    await user.click(await screen.findByRole('button', { name: 'Connect to staging' }));
    const dialog = await screen.findByRole('dialog', { name: 'Connect to staging' });
    await user.type(within(dialog).getByLabelText('Password'), 'wrong');
    await user.click(within(dialog).getByRole('button', { name: 'Connect' }));

    expect(await screen.findByRole('heading', { name: 'Couldn’t connect' })).toBeInTheDocument();
    expect(screen.getByText('Authentication failed.')).toBeInTheDocument();
    await waitFor(() =>
      expect(container.querySelector('.gem-log-row[data-kind="error"]')).toHaveTextContent(
        'Authentication failed (password)',
      ),
    );
    // The password itself never reaches the log.
    expect(container.querySelector('.gem-log')?.textContent).not.toContain('wrong');
  });

  it('blocks a changed host key and shows both fingerprints (CA-08)', async () => {
    const user = userEvent.setup();
    renderApp();
    await user.click(await screen.findByRole('button', { name: 'Connect to backup-box' }));
    const dialog = await screen.findByRole('alertdialog', { name: /Server identity changed/ });
    expect(within(dialog).getByText('Stored fingerprint')).toBeInTheDocument();
    expect(within(dialog).getByText(/^SHA256:0ldK3y/)).toBeInTheDocument();
    expect(within(dialog).getByText(/^SHA256:N3wK3y/)).toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: /trust|connect/i })).toBeNull();
    expect(within(dialog).getByRole('button', { name: 'Open site settings' })).toBeInTheDocument();
  });
});

describe('viewer', () => {
  it('previews with Space (italic tab, replaced by the next preview) and pins with Enter', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);

    await user.click(await row('config.yaml'));
    await user.keyboard(' ');
    const preview = await screen.findByRole('button', { name: 'config.yaml' });
    expect(preview.querySelector('[data-preview]')).not.toBeNull();
    await editorFor('config.yaml');

    // The next preview replaces it.
    await user.keyboard('{ArrowDown} ');
    const next = await screen.findByRole('button', { name: 'deploy.sh' });
    expect(next.querySelector('[data-preview]')).not.toBeNull();
    expect(screen.queryByRole('button', { name: 'config.yaml' })).toBeNull();

    // Enter opens it for good.
    await user.keyboard('{Enter}');
    await waitFor(() => expect(next.querySelector('[data-preview]')).toBeNull());

    // A new preview opens beside the pinned tab.
    await user.keyboard('{ArrowUp} ');
    expect(await screen.findByRole('button', { name: 'config.yaml' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'deploy.sh' })).toBeInTheDocument();
  });

  it('detects the language and shows the footer facts', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.dblClick(await row('index.php'));
    await editorFor('index.php');
    const footer = screen.getByRole('contentinfo', { name: 'File details' });
    expect(within(footer).getByRole('combobox', { name: 'Language' })).toHaveValue('php');
    expect(footer).toHaveTextContent('UTF-8');
    expect(footer).toHaveTextContent('LF');
    expect(footer).toHaveTextContent('Ln 1, Col 1');
  });

  it('edits, saves, and asks on a conflict; Overwrite saves anyway (CA-04, CA-05)', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');

    view.dispatch({ changes: { from: 0, insert: '# edited in gem\n' } });
    // The file tab and its connection tab both show the unsaved square.
    expect(await screen.findAllByLabelText('Unsaved changes')).toHaveLength(2);

    // A save without conflict.
    await user.keyboard('{Control>}s{/Control}');
    expect(await screen.findByText('Saved')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByLabelText('Unsaved changes')).toBeNull());
    const file = () => controls.tree.get('/var/www/api/config.yaml') as { text: string };
    expect(file().text.startsWith('# edited in gem\n')).toBe(true);

    // The file changes on the server; the next save asks.
    controls.changeOnServer('/var/www/api/config.yaml', '# from the server\n');
    view.dispatch({ changes: { from: 0, insert: '# second edit\n' } });
    await user.keyboard('{Control>}s{/Control}');
    const dialog = await screen.findByRole('alertdialog', {
      name: 'The file changed on the server',
    });
    expect(
      within(dialog).getByRole('button', { name: 'Reload (discard mine)' }),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Overwrite' }));
    await waitFor(() => expect(file().text.startsWith('# second edit\n')).toBe(true));
    expect(file().text).not.toContain('# from the server');
  });

  it('keeps the window-wide shortcuts working while the editor has the focus', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');
    view.focus();
    expect(view.hasFocus).toBe(true);

    await user.keyboard('{Control>}t{/Control}');
    expect(await screen.findByRole('button', { name: 'Close Sites' })).toBeInTheDocument();

    await user.keyboard('{Control>}1{/Control}');
    (await editorFor('config.yaml')).focus();
    await user.keyboard('{Control>},{/Control}');
    expect(await screen.findByRole('dialog', { name: 'Settings' })).toBeInTheDocument();
    // The editor text was not touched by the shortcuts.
    expect(view.state.doc.toString()).not.toMatch(/^[t1,]/);
  });

  it('Reload on a conflict discards the local edit', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');
    controls.changeOnServer('/var/www/api/config.yaml', '# from the server\n');
    view.dispatch({ changes: { from: 0, insert: '# mine\n' } });
    await user.keyboard('{Control>}s{/Control}');
    const dialog = await screen.findByRole('alertdialog', {
      name: 'The file changed on the server',
    });
    await user.click(within(dialog).getByRole('button', { name: 'Reload (discard mine)' }));
    await waitFor(async () => {
      const current = await editorFor('config.yaml');
      expect(current.state.doc.toString()).toContain('# from the server');
      expect(current.state.doc.toString()).not.toContain('# mine');
    });
    expect(screen.queryByLabelText('Unsaved changes')).toBeNull();
  });

  it('asks before overwriting in place where a temp file is not allowed', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/etc/nginx/sites-available{Enter}');
    await user.dblClick(await row('default'));
    const view = await editorFor('default');
    expect(
      within(screen.getByRole('contentinfo', { name: 'File details' })).getByRole('combobox', {
        name: 'Language',
      }),
    ).toHaveValue('nginx');
    view.dispatch({ changes: { from: 0, insert: '# tuned\n' } });
    await user.keyboard('{Control>}s{/Control}');
    const dialog = await screen.findByRole('alertdialog', { name: 'Overwrite in place?' });
    await user.click(within(dialog).getByRole('button', { name: 'Overwrite in place' }));
    expect(await screen.findByText('Saved (overwritten in place)')).toBeInTheDocument();
  });

  it('keeps partial files read-only and loads more on demand (CA-03)', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/var/www/api/storage/logs{Enter}');
    await user.dblClick(await row('access.log'));
    await editorFor('access.log');
    expect(await screen.findByText(/^Showing 2\.0 MB of 4\.\d MB$/)).toBeInTheDocument();
    expect(screen.getByText(/partially loaded/)).toBeInTheDocument();
    const saveFile = vi.spyOn(api, 'saveFile');
    await user.keyboard('{Control>}s{/Control}');
    expect(saveFile).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Load more' }));
    expect(await screen.findByText(/^Showing 4\.0 MB of 4\.\d MB$/)).toBeInTheDocument();
  });

  it('asks before loading a file above the threshold', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/var/www/api/storage/logs{Enter}');
    await user.click(await row('app.log'));
    await user.keyboard(' ');
    expect(await screen.findByRole('heading', { name: 'This file is 200 MB' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Open' }));
    expect(await screen.findByText('Showing 2.0 MB of 200 MB')).toBeInTheDocument();
  });

  it('shows binary files as an empty state with Download', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/var/www/api/public/assets{Enter}');
    await user.dblClick(await row('logo.png'));
    expect(await screen.findByRole('heading', { name: 'Binary file' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Download' })).toBeInTheDocument();
  });
});

describe('file list', () => {
  async function deleteFlow(user: User, controls: MockControls, confirm: boolean) {
    await connectProd(user);
    await user.click(await row('go.mod'));
    await user.keyboard('{Delete}');
    const dialog = await screen.findByRole('alertdialog', { name: 'Delete from the server?' });
    expect(dialog).toHaveTextContent('“go.mod” will be deleted from the server');
    await user.click(within(dialog).getByRole('button', { name: confirm ? 'Delete' : 'Cancel' }));
    return controls.tree.get('/var/www/api/go.mod');
  }

  it('deletes after the confirmation and reports it', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    expect(await deleteFlow(user, controls, true)).toBeUndefined();
    expect(await screen.findByText('Deleted 1 item')).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole('option', { name: 'go.mod' })).toBeNull());
  });

  it('keeps the file when the confirmation is cancelled', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    expect(await deleteFlow(user, controls, false)).toBeDefined();
    expect(screen.getByRole('option', { name: 'go.mod' })).toBeInTheDocument();
  });

  it('filters by typing, and shows permission errors in place', async () => {
    const user = userEvent.setup();
    renderApp();
    const list = await connectProd(user);
    list.focus();
    await user.keyboard('compo');
    expect(screen.getByRole('textbox', { name: 'Filter this folder' })).toHaveValue('compo');
    await waitFor(() =>
      expect(
        within(list)
          .getAllByRole('option')
          .map((option) => option.getAttribute('aria-label')),
      ).toEqual(['composer.json', 'composer.lock', 'docker-compose.yml']),
    );

    await user.keyboard('{Escape}');
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/var/www/api/private{Enter}');
    expect(await screen.findByRole('heading', { name: 'Permission denied' })).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Go to the parent folder' }));
    expect(
      await screen.findByRole('listbox', { name: 'Files in /var/www/api' }),
    ).toBeInTheDocument();
  });

  it('renders a 10,000-entry folder through the virtual list', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.keyboard('{Control>}l{/Control}');
    await user.keyboard('{Control>}a{/Control}/var/cache/thumbs{Enter}');
    const list = await screen.findByRole('listbox', { name: 'Files in /var/cache/thumbs' });
    await within(list).findByRole('option', { name: /^thumb-00000-/ });
    expect(within(list).getAllByRole('option').length).toBeLessThan(60);
    expect(screen.getByRole('contentinfo')).toHaveTextContent('10,000 items');
    await user.keyboard('{End}');
    expect(await within(list).findByRole('option', { name: /^thumb-09999-/ })).toBeInTheDocument();
  });
});

describe('site dialog', () => {
  it('validates inline and keeps the port in step with the protocol', async () => {
    const user = userEvent.setup();
    renderApp({ mode: 'empty' });
    expect(await screen.findByRole('heading', { name: 'No sites yet' })).toBeInTheDocument();
    const createSite = vi.spyOn(api, 'createSite');
    await user.click(screen.getAllByRole('button', { name: 'New site' })[0]!);
    const dialog = await screen.findByRole('dialog', { name: 'New site' });

    await user.click(within(dialog).getByRole('button', { name: 'Save site' }));
    expect(within(dialog).getByText('Enter a host name or address')).toBeInTheDocument();
    expect(createSite).not.toHaveBeenCalled();

    const port = within(dialog).getByLabelText('Port');
    expect(port).toHaveValue('22');
    await user.click(within(dialog).getByRole('button', { name: 'FTP' }));
    expect(port).toHaveValue('21');
    expect(
      within(dialog).getByText(/sends the password and every file unencrypted/),
    ).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'FTPS' }));
    await user.click(within(dialog).getByRole('button', { name: 'Implicit (port 990)' }));
    expect(port).toHaveValue('990');
    await user.clear(port);
    await user.type(port, '99999');
    await user.type(within(dialog).getByLabelText(/^Host/), 'files.example.test');
    await user.click(within(dialog).getByRole('button', { name: 'Save site' }));
    expect(within(dialog).getByText('Use a port from 1 to 65535')).toBeInTheDocument();

    await user.clear(port);
    await user.type(port, '990');
    await user.click(within(dialog).getByRole('button', { name: 'Save site' }));
    await waitFor(() => expect(screen.queryByRole('dialog', { name: 'New site' })).toBeNull());
    expect(createSite).toHaveBeenCalledWith(
      expect.objectContaining({
        host: 'files.example.test',
        protocol: 'ftps',
        port: 990,
        name: 'files.example.test',
      }),
      null,
    );
    expect(
      await screen.findByRole('button', { name: 'Connect to files.example.test' }),
    ).toBeInTheDocument();
  });

  it('opens quick connect targets in a tab and refuses bad addresses', async () => {
    const user = userEvent.setup();
    renderApp();
    const field = await screen.findByLabelText('Quick connect');
    await user.type(field, 'gopher://example.com');
    await user.click(screen.getByRole('button', { name: 'Connect' }));
    expect(screen.getByRole('alert')).toHaveTextContent('Use sftp://, ftp://, ftps:// or ftpes://');
    await user.clear(field);
    await user.type(field, 'sftp://ops@new.example.test/srv');
    await user.click(screen.getByRole('button', { name: 'Connect' }));
    expect(
      await screen.findByRole('alertdialog', { name: 'Trust this server?' }),
    ).toHaveTextContent('new.example.test:22');
    const tabs = screen.getByRole('navigation', { name: 'Connections' });
    expect(within(tabs).getByText('ops@new.example.test')).toBeInTheDocument();
  });
});

describe('sites page', () => {
  it('connects on a double click on a site row, and on Enter', async () => {
    const user = userEvent.setup();
    renderApp();
    const row = await screen.findByRole('button', { name: /^prod-api,/ });
    await user.click(row);
    expect(row).toHaveAttribute('data-selected');
    expect(screen.queryByRole('alertdialog')).toBeNull();
    await user.dblClick(row);
    expect(
      await screen.findByRole('alertdialog', { name: 'Trust this server?' }),
    ).toBeInTheDocument();
    // Exactly one connection tab was opened.
    const tabs = screen.getByRole('navigation', { name: 'Connections' });
    expect(within(tabs).getAllByText('prod-api')).toHaveLength(1);
  });

  it('connects with Enter on the focused row', async () => {
    const user = userEvent.setup();
    renderApp();
    const row = await screen.findByRole('button', { name: /^staging,/ });
    row.focus();
    await user.keyboard('{Enter}');
    expect(await screen.findByRole('dialog', { name: 'Connect to staging' })).toBeInTheDocument();
  });
});

describe('prompts that open mid-gesture', () => {
  it('a held Enter that started the connection does not answer the host-key prompt', async () => {
    renderApp();
    const row = await screen.findByRole('button', { name: /^prod-api,/ });
    row.focus();
    fireEvent.keyDown(row, { key: 'Enter' });
    const dialog = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
    const cancel = within(dialog).getByRole('button', { name: 'Cancel' });
    await waitFor(() => expect(cancel).toHaveFocus());
    // The key auto-repeats onto the autofocused Cancel, then is released.
    fireEvent.keyDown(cancel, { key: 'Enter', repeat: true });
    fireEvent.click(cancel, { detail: 0 });
    fireEvent.keyUp(cancel, { key: 'Enter' });
    // A spurious close request (no Escape since the dialog opened) is ignored too.
    fireEvent(dialog, new Event('cancel', { cancelable: true }));
    expect(screen.getByRole('alertdialog', { name: 'Trust this server?' })).toBeInTheDocument();
    expect(screen.queryByText('Connection cancelled.')).toBeNull();

    // A deliberate answer still works.
    fireEvent.keyDown(cancel, { key: 'Enter' });
    fireEvent.click(cancel, { detail: 0 });
    expect(await screen.findByText('Connection cancelled.')).toBeInTheDocument();
  });
});

describe('site dialog: test connection', () => {
  async function openNewSite(user: User) {
    await user.click(await screen.findByRole('button', { name: 'New site' }));
    return screen.findByRole('dialog', { name: 'New site' });
  }

  it('shows the conversation and offers to trust an unknown host key', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    const dialog = await openNewSite(user);
    await user.type(within(dialog).getByLabelText(/^Host/), 'prod.example.com');
    await user.click(within(dialog).getByRole('button', { name: 'Agent' }));
    await user.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('is not trusted yet');
    expect(within(dialog).getByText(/^SHA256:pV6m2Ug0/)).toBeInTheDocument();
    const log = within(dialog).getByLabelText('Server conversation');
    expect(log).toHaveTextContent('Resolving prod.example.com');
    expect(log).toHaveTextContent('Test failed:');

    await user.click(within(dialog).getByRole('button', { name: 'Trust and test again' }));
    expect(await within(dialog).findByText(/^Connected\. Home folder:/)).toBeInTheDocument();
    expect(controls.knownHosts.has('prod.example.com:22')).toBe(true);
    expect(within(dialog).getByLabelText('Server conversation')).toHaveTextContent(
      'Authenticated as',
    );
  });

  it('focuses the password field when the server asks for one, and logs a failed login', async () => {
    const user = userEvent.setup();
    renderApp();
    const dialog = await openNewSite(user);
    await user.type(within(dialog).getByLabelText(/^Host/), 'staging.example.com');
    const port = within(dialog).getByLabelText('Port');
    await user.clear(port);
    await user.type(port, '2222');
    await user.type(within(dialog).getByLabelText('User'), 'deploy');
    await user.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Enter the password');
    const password = within(dialog).getByLabelText('Password');
    await waitFor(() => expect(password).toHaveFocus());

    await user.type(password, 'wrong');
    await user.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    expect(await within(dialog).findByRole('alert')).toHaveTextContent('Authentication failed.');
    const log = within(dialog).getByLabelText('Server conversation');
    expect(log.querySelector('[data-kind="error"]')).toHaveTextContent('Authentication failed');
    expect(log.textContent).not.toContain('wrong');
  });

  it('shows both fingerprints of a changed host key, with no way to trust it', async () => {
    const user = userEvent.setup();
    renderApp();
    const row = await screen.findByRole('button', { name: /^backup-box,/ });
    fireEvent.contextMenu(row);
    await user.click(await screen.findByRole('menuitem', { name: /^Edit…/ }));
    const dialog = await screen.findByRole('dialog', { name: 'Edit site' });
    await user.click(within(dialog).getByRole('button', { name: 'Test connection' }));

    const alert = await within(dialog).findByRole('alert');
    expect(alert).toHaveTextContent('changed');
    expect(within(alert).getByText(/^SHA256:0ldK3y/)).toBeInTheDocument();
    expect(within(alert).getByText(/^SHA256:N3wK3y/)).toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: /^Trust/ })).toBeNull();

    // Forgetting the stored key turns it into an unknown key, which can then be trusted.
    await user.click(within(dialog).getByRole('button', { name: 'Forget host key' }));
    await user.click(within(dialog).getByRole('button', { name: 'Test connection' }));
    expect(
      await within(dialog).findByRole('button', { name: 'Trust and test again' }),
    ).toBeInTheDocument();
  });
});

describe('polish', () => {
  it('Mod+W closes the file tab from the editor (asking when dirty), else the connection tab', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');
    view.focus();
    await user.keyboard('{Control>}w{/Control}');
    await waitFor(() => expect(screen.queryByRole('button', { name: 'config.yaml' })).toBeNull());
    const tabs = screen.getByRole('navigation', { name: 'Connections' });
    expect(within(tabs).getByText('prod-api')).toBeInTheDocument();

    // A dirty file asks first.
    await user.dblClick(await row('index.php'));
    const php = await editorFor('index.php');
    php.dispatch({ changes: { from: 0, insert: '// x\n' } });
    php.focus();
    await user.keyboard('{Control>}w{/Control}');
    const ask = await screen.findByRole('alertdialog', { name: 'Save changes?' });
    await user.click(within(ask).getByRole('button', { name: 'Discard changes' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'index.php' })).toBeNull());

    // From the list, Mod+W closes the connection tab.
    screen.getByRole('listbox', { name: 'Files in /var/www/api' }).focus();
    await user.keyboard('{Control>}w{/Control}');
    await waitFor(() =>
      expect(screen.queryByRole('navigation', { name: 'Connections' })).toBeNull(),
    );
  });

  it('Mod+L edits the path from the editor too', async () => {
    const user = userEvent.setup();
    renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');
    view.focus();
    await user.keyboard('{Control>}l{/Control}');
    const input = await screen.findByRole('textbox', { name: 'Remote path' });
    expect(input).toHaveFocus();
    expect(input).toHaveValue('/var/www/api');
    // CodeMirror did not select a line.
    expect(view.state.selection.main.empty).toBe(true);
  });

  it('shows the fresh size in the list after a conflict reload', async () => {
    const user = userEvent.setup();
    const { controls } = renderApp();
    await connectProd(user);
    await user.dblClick(await row('config.yaml'));
    const view = await editorFor('config.yaml');
    controls.changeOnServer('/var/www/api/config.yaml', '# a longer line from the server\n');
    const size = (controls.tree.get('/var/www/api/config.yaml') as { size: number }).size;
    view.dispatch({ changes: { from: 0, insert: '# mine\n' } });
    await user.keyboard('{Control>}s{/Control}');
    const dialog = await screen.findByRole('alertdialog', {
      name: 'The file changed on the server',
    });
    await user.click(within(dialog).getByRole('button', { name: 'Reload (discard mine)' }));
    await waitFor(() =>
      expect(
        screen.getByRole('option', { name: 'config.yaml' }).querySelector('.gem-file-size'),
      ).toHaveTextContent(`${(size / 1024).toFixed(1)} KB`),
    );
  });

  it('without a keyring, nothing offers to remember a password', async () => {
    const user = userEvent.setup();
    renderApp({ keyring: false });
    // The footer fact shares its element with the protocol list, so match part of the text.
    expect(await screen.findAllByText(/No system keyring: passwords are asked/)).not.toHaveLength(
      0,
    );
    expect(screen.queryAllByText(/Passwords in the system keyring/)).toHaveLength(0);
    await user.click(await screen.findByRole('button', { name: 'New site' }));
    const dialog = await screen.findByRole('dialog', { name: 'New site' });
    const remember = await within(dialog).findByRole('checkbox', {
      name: 'Remember in the system keyring',
    });
    await waitFor(() => expect(remember).toBeDisabled());
    expect(remember).not.toBeChecked();
    expect(remember).toHaveAccessibleDescription(/keyring isn’t available/);
    await user.click(within(dialog).getByRole('button', { name: 'Cancel' }));

    await user.click(await screen.findByRole('button', { name: 'Connect to staging' }));
    const prompt = await screen.findByRole('dialog', { name: 'Connect to staging' });
    const promptRemember = within(prompt).getByRole('checkbox');
    expect(promptRemember).toBeDisabled();
    expect(promptRemember).not.toBeChecked();
    expect(within(prompt).getByText(/keyring isn’t available/)).toBeInTheDocument();
  });

  it('gives two quick-connect tabs to the same server different names', async () => {
    const user = userEvent.setup();
    renderApp();
    const quick = async () => {
      await user.type(await screen.findByLabelText('Quick connect'), 'sftp://gem:secret@127.0.0.1');
      await user.click(screen.getByRole('button', { name: 'Connect' }));
    };
    await quick();
    const trust = await screen.findByRole('alertdialog', { name: 'Trust this server?' });
    await user.click(within(trust).getByRole('button', { name: 'Trust and connect' }));
    await screen.findByRole('listbox', { name: /^Files in/ });
    await user.keyboard('{Control>}t{/Control}');
    await quick();
    const tabs = screen.getByRole('navigation', { name: 'Connections' });
    await waitFor(() => expect(within(tabs).getByText('gem@127.0.0.1 (2)')).toBeInTheDocument());
    expect(within(tabs).getByText('gem@127.0.0.1')).toBeInTheDocument();
  });
});

describe('updates (ADR 0015)', () => {
  const offer = () => {
    const update = {
      version: '0.2.0',
      close: vi.fn(() => Promise.resolve()),
      downloadAndInstall: vi.fn(() => Promise.resolve()),
    };
    useUpdates.setState({ update: update as unknown as Update });
    return update;
  };

  it('offers a found update, and "Later" dismisses it', async () => {
    const user = userEvent.setup();
    renderApp();
    const update = offer();
    const dialog = await screen.findByRole('dialog', { name: 'Gem 0.2.0 is available' });
    await user.click(within(dialog).getByRole('button', { name: 'Later' }));
    expect(screen.queryByRole('dialog', { name: /is available/ })).not.toBeInTheDocument();
    expect(update.close).toHaveBeenCalled();
  });

  it('does not install while transfers are running', async () => {
    const user = userEvent.setup();
    renderApp();
    useSessionData.setState({
      transfers: {
        tab: [
          {
            id: 't1',
            sessionId: 's1',
            direction: 'download',
            name: 'big.iso',
            localPath: '/tmp/big.iso',
            remotePath: '/big.iso',
            size: 100,
            done: 10,
            state: 'running',
            error: null,
            startedAt: 0,
            finishedAt: null,
          },
        ],
      },
    });
    const update = offer();
    const dialog = await screen.findByRole('dialog', { name: 'Gem 0.2.0 is available' });
    await user.click(within(dialog).getByRole('button', { name: 'Install and restart' }));
    expect(within(dialog).getByRole('alert')).toHaveTextContent('Wait for the transfers to finish');
    expect(update.downloadAndInstall).not.toHaveBeenCalled();
  });
});
