import { describe, expect, it } from 'vitest';
import { parseQuickConnect } from './quickConnect';

function ok(input: string) {
  const result = parseQuickConnect(input);
  if (!result.ok) throw new Error(`expected ${input} to parse, got ${result.error}`);
  return result;
}

describe('parseQuickConnect', () => {
  it('parses a full SFTP URL', () => {
    const { site, password } = ok('sftp://deploy@prod.example.com:2222/var/www/api');
    expect(site).toMatchObject({
      protocol: 'sftp',
      host: 'prod.example.com',
      port: 2222,
      user: 'deploy',
      auth: 'password',
      initialPath: '/var/www/api',
      name: 'deploy@prod.example.com',
    });
    expect(password).toBeNull();
  });

  it('defaults to SFTP on port 22 without a scheme', () => {
    expect(ok('deploy@example.com').site).toMatchObject({ protocol: 'sftp', port: 22 });
    expect(ok('example.com').site).toMatchObject({ host: 'example.com', user: '', port: 22 });
  });

  it('maps schemes to protocols and default ports', () => {
    expect(ok('ftp://example.com').site).toMatchObject({ protocol: 'ftp', port: 21 });
    expect(ok('ftps://example.com').site).toMatchObject({
      protocol: 'ftps',
      ftpsMode: 'implicit',
      port: 990,
    });
    expect(ok('ftpes://example.com').site).toMatchObject({
      protocol: 'ftps',
      ftpsMode: 'explicit',
      port: 21,
    });
    expect(ok('SFTP://Example.com').site).toMatchObject({ protocol: 'sftp', host: 'Example.com' });
    expect(ok('ssh://example.com').site.protocol).toBe('sftp');
  });

  it('keeps a password apart, decoded, for this connection only', () => {
    const { site, password } = ok('ftp://user:p%40ss%3Aword@example.com/');
    expect(site.user).toBe('user');
    expect(password).toBe('p@ss:word');
    expect(JSON.stringify(site)).not.toContain('p@ss');
  });

  it('decodes user names and paths', () => {
    const { site } = ok('sftp://first%20last@example.com/srv/my%20site');
    expect(site.user).toBe('first last');
    expect(site.initialPath).toBe('/srv/my site');
  });

  it('accepts an @ inside the user name (the last @ splits)', () => {
    expect(ok('sftp://me@corp@example.com').site).toMatchObject({
      user: 'me@corp',
      host: 'example.com',
    });
  });

  it('supports IPv6 literals', () => {
    expect(ok('sftp://root@[2001:db8::1]:2200/').site).toMatchObject({
      host: '2001:db8::1',
      port: 2200,
    });
    expect(ok('[::1]').site).toMatchObject({ host: '::1', port: 22 });
  });

  it('normalises the path and drops a bare /', () => {
    expect(ok('sftp://h/var//www/./api/').site.initialPath).toBe('/var/www/api');
    expect(ok('sftp://h/').site.initialPath).toBeNull();
    expect(ok('sftp://h').site.initialPath).toBeNull();
  });

  it('trims surrounding spaces', () => {
    expect(ok('  sftp://h:22  ').site.host).toBe('h');
  });

  it.each([
    ['', 'empty'],
    ['   ', 'empty'],
    ['http://example.com', 'scheme'],
    ['sftp://', 'host'],
    ['sftp://user@', 'host'],
    ['sftp://:22', 'host'],
    ['sftp://[::1', 'host'],
    ['sftp://example.com:http', 'port'],
    ['sftp://example.com:0', 'port'],
    ['sftp://example.com:65536', 'port'],
    ['sftp://exa mple.com', 'host'],
  ] as const)('refuses %j (%s)', (input, error) => {
    expect(parseQuickConnect(input)).toEqual({ ok: false, error });
  });
});
