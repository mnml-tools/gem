import { describe, expect, it } from 'vitest';
import { siteDialogStrings } from '@/strings/sites';
import {
  formFromInput,
  needsSecretField,
  newForm,
  secretForCreate,
  secretForUpdate,
  toSiteInput,
  validateSite,
  withProtocol,
} from './siteForm';

const messages = siteDialogStrings.errors;

describe('site form', () => {
  it('starts as an SFTP site on port 22 with password auth', () => {
    expect(newForm()).toMatchObject({
      protocol: 'sftp',
      port: '22',
      auth: 'password',
      passive: true,
    });
  });

  it('moves the port with the protocol until the user types one', () => {
    let form = newForm();
    form = withProtocol(form, 'ftp');
    expect(form.port).toBe('21');
    form = withProtocol(form, 'ftps', 'implicit');
    expect(form.port).toBe('990');
    form = { ...form, port: '2121', portEdited: true };
    form = withProtocol(form, 'sftp');
    expect(form.port).toBe('2121');
  });

  it('keeps key and agent auth for SFTP only', () => {
    const form = withProtocol({ ...newForm(), auth: 'agent' }, 'ftp');
    expect(form.auth).toBe('password');
    expect(needsSecretField({ ...newForm(), auth: 'agent' })).toBe(false);
    expect(needsSecretField({ ...newForm(), auth: 'key' })).toBe(true);
    expect(needsSecretField(withProtocol(newForm(), 'ftp'))).toBe(true);
  });

  it('treats a non-default saved port as typed', () => {
    const form = formFromInput({ ...toSiteInput({ ...newForm(), host: 'h' }), port: 2222 });
    expect(form.portEdited).toBe(true);
    expect(withProtocol(form, 'ftp').port).toBe('2222');
  });

  it('validates host, port, key file and initial path', () => {
    expect(validateSite(newForm(), messages)).toEqual({ host: messages.host });
    const valid = { ...newForm(), host: 'example.com' };
    expect(validateSite(valid, messages)).toEqual({});
    expect(validateSite({ ...valid, host: 'user@example.com' }, messages).host).toBe(messages.host);
    expect(validateSite({ ...valid, port: '0' }, messages).port).toBe(messages.port);
    expect(validateSite({ ...valid, port: '65536' }, messages).port).toBe(messages.port);
    expect(validateSite({ ...valid, port: '22a' }, messages).port).toBe(messages.port);
    expect(validateSite({ ...valid, auth: 'key' }, messages).keyPath).toBe(messages.keyPath);
    expect(validateSite({ ...valid, auth: 'key', keyPath: '/k' }, messages)).toEqual({});
    expect(validateSite({ ...valid, initialPath: 'var/www' }, messages).initialPath).toBe(
      messages.initialPath,
    );
    expect(validateSite({ ...valid, initialPath: '~/www' }, messages)).toEqual({});
  });

  it('builds the IPC input: name defaults to the host, no key path unless key auth', () => {
    const input = toSiteInput({
      ...newForm(),
      host: ' example.com ',
      keyPath: '/k',
      initialPath: '  ',
      user: ' deploy ',
    });
    expect(input).toMatchObject({
      name: 'example.com',
      host: 'example.com',
      user: 'deploy',
      keyPath: null,
      initialPath: null,
      port: 22,
    });
    expect(toSiteInput({ ...newForm(), host: 'h', auth: 'key', keyPath: '/k' }).keyPath).toBe('/k');
    expect(toSiteInput({ ...withProtocol(newForm(), 'ftp'), host: 'h', auth: 'agent' }).auth).toBe(
      'password',
    );
  });

  it('sends a secret on create only when typed and remembered', () => {
    const form = { ...newForm(), host: 'h', secret: 'pw' };
    expect(secretForCreate(form)).toBe('pw');
    expect(secretForCreate({ ...form, remember: false })).toBeNull();
    expect(secretForCreate({ ...form, auth: 'agent' })).toBeNull();
  });

  it('maps Keep / Change / Clear to SecretUpdate', () => {
    const form = { ...newForm(), host: 'h' };
    expect(secretForUpdate({ ...form, secretMode: 'keep', secret: 'x' }, true)).toEqual({
      kind: 'keep',
    });
    expect(secretForUpdate({ ...form, secretMode: 'change', secret: 'new' }, true)).toEqual({
      kind: 'set',
      value: 'new',
    });
    expect(secretForUpdate({ ...form, secretMode: 'change', secret: '' }, true)).toEqual({
      kind: 'keep',
    });
    expect(secretForUpdate({ ...form, secretMode: 'clear' }, true)).toEqual({ kind: 'clear' });
    // No stored secret: typing one with Remember stores it.
    expect(secretForUpdate({ ...form, secret: 'pw' }, false)).toEqual({ kind: 'set', value: 'pw' });
    expect(secretForUpdate({ ...form, secret: 'pw', remember: false }, false)).toEqual({
      kind: 'keep',
    });
    // Switching to agent auth drops a stored password.
    expect(secretForUpdate({ ...form, auth: 'agent' }, true)).toEqual({ kind: 'clear' });
  });
});
