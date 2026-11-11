/**
 * Parses the Sites page's quick-connect field: `sftp://user@host:port/path`, `ftp://…`,
 * `ftps://…` (implicit TLS), `ftpes://…` (explicit TLS), or a bare `user@host[:port][/path]`
 * (SFTP). A password in the URL is returned separately, used for this connection only and
 * never saved.
 */
import type { FtpsMode } from './ipc/bindings/FtpsMode';
import type { Protocol } from './ipc/bindings/Protocol';
import type { SiteInput } from './ipc/bindings/SiteInput';
import { normalizePath } from './paths';
import { defaultPort } from './sites';

export type QuickConnectError = 'empty' | 'scheme' | 'host' | 'port';

export type QuickConnectResult =
  { ok: true; site: SiteInput; password: string | null } | { ok: false; error: QuickConnectError };

const SCHEMES: Record<string, { protocol: Protocol; ftpsMode: FtpsMode }> = {
  sftp: { protocol: 'sftp', ftpsMode: 'explicit' },
  ssh: { protocol: 'sftp', ftpsMode: 'explicit' },
  ftp: { protocol: 'ftp', ftpsMode: 'explicit' },
  ftps: { protocol: 'ftps', ftpsMode: 'implicit' },
  ftpes: { protocol: 'ftps', ftpsMode: 'explicit' },
};

function decode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

export function parseQuickConnect(input: string): QuickConnectResult {
  let rest = input.trim();
  if (!rest) return { ok: false, error: 'empty' };

  let scheme = SCHEMES.sftp!;
  const schemeMatch = /^([a-z][a-z0-9+.-]*):\/\//i.exec(rest);
  if (schemeMatch) {
    const found = SCHEMES[schemeMatch[1]!.toLowerCase()];
    if (!found) return { ok: false, error: 'scheme' };
    scheme = found;
    rest = rest.slice(schemeMatch[0].length);
  }

  // The authority ends at the first `/`; the user part ends at its last `@`.
  const slash = rest.indexOf('/');
  const authority = slash >= 0 ? rest.slice(0, slash) : rest;
  const path = slash >= 0 ? rest.slice(slash) : '';
  const at = authority.lastIndexOf('@');
  const userInfo = at >= 0 ? authority.slice(0, at) : '';
  const hostPort = authority.slice(at + 1);

  let host = hostPort;
  let portText = '';
  if (hostPort.startsWith('[')) {
    const close = hostPort.indexOf(']');
    if (close < 0) return { ok: false, error: 'host' };
    host = hostPort.slice(1, close);
    const after = hostPort.slice(close + 1);
    if (after && !after.startsWith(':')) return { ok: false, error: 'host' };
    portText = after.slice(1);
  } else {
    const colon = hostPort.lastIndexOf(':');
    if (colon >= 0) {
      host = hostPort.slice(0, colon);
      portText = hostPort.slice(colon + 1);
    }
  }
  host = host.trim();
  if (!host || /[\s/@]/.test(host)) return { ok: false, error: 'host' };

  let port = defaultPort(scheme.protocol, scheme.ftpsMode);
  if (portText) {
    if (!/^\d{1,5}$/.test(portText)) return { ok: false, error: 'port' };
    port = Number(portText);
    if (port < 1 || port > 65535) return { ok: false, error: 'port' };
  }

  let user = userInfo;
  let password: string | null = null;
  const colon = userInfo.indexOf(':');
  if (colon >= 0) {
    user = userInfo.slice(0, colon);
    password = decode(userInfo.slice(colon + 1));
  }
  user = decode(user);

  const decodedPath = decode(path);
  const initialPath = decodedPath && decodedPath !== '/' ? normalizePath(decodedPath) : null;

  return {
    ok: true,
    password,
    site: {
      name: `${user ? `${user}@` : ''}${host}`,
      protocol: scheme.protocol,
      host,
      port,
      user,
      auth: 'password',
      keyPath: null,
      ftpsMode: scheme.ftpsMode,
      passive: true,
      initialPath,
      color: null,
    },
  };
}
