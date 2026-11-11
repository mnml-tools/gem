import { describe, expect, it } from 'vitest';
import type { ConnectionTab } from './workspace';
import { tabLabels } from './workspace';

const tab = (id: string, label: string, port = 22, protocol: ConnectionTab['protocol'] = 'sftp') =>
  ({ id, kind: 'connection', label, port, protocol }) as ConnectionTab;

describe('tabLabels', () => {
  it('keeps unique labels as they are', () => {
    const labels = tabLabels([
      tab('a', 'prod-api'),
      tab('b', 'staging'),
      { id: 's', kind: 'sites' },
    ]);
    expect([...labels.values()]).toEqual(['prod-api', 'staging']);
  });

  it('adds the port when the ports differ, the protocol when that differs', () => {
    const labels = tabLabels([
      tab('a', 'gem@127.0.0.1', 2222),
      tab('b', 'gem@127.0.0.1', 2223),
      tab('c', 'ops@box', 21, 'ftp'),
      tab('d', 'ops@box', 21, 'ftps'),
    ]);
    expect(labels.get('a')).toBe('gem@127.0.0.1:2222');
    expect(labels.get('b')).toBe('gem@127.0.0.1:2223');
    expect(labels.get('c')).toBe('ops@box · FTP');
    expect(labels.get('d')).toBe('ops@box · FTPS');
  });

  it('numbers tabs that are still the same', () => {
    const labels = tabLabels([
      tab('a', 'gem@127.0.0.1'),
      tab('b', 'gem@127.0.0.1'),
      tab('c', 'gem@127.0.0.1'),
    ]);
    expect([labels.get('a'), labels.get('b'), labels.get('c')]).toEqual([
      'gem@127.0.0.1',
      'gem@127.0.0.1 (2)',
      'gem@127.0.0.1 (3)',
    ]);
  });
});
