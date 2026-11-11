import { beforeEach, describe, expect, it } from 'vitest';
import type { OpenedFile } from '@/lib/ipc/bindings/OpenedFile';
import { dirtyFileTabs, getFileTabs, useFileTabs } from './fileTabs';

const CONN = 'conn-1';
const store = () => useFileTabs.getState();
const tabs = () => getFileTabs(CONN).tabs.map((tab) => `${tab.name}${tab.preview ? '*' : ''}`);

const opened = (path: string, text = 'hello\n'): OpenedFile => ({
  path,
  name: path.slice(path.lastIndexOf('/') + 1),
  size: text.length,
  mtime: 1000,
  text,
  nextOffset: text.length,
  complete: true,
  binary: false,
  encoding: 'utf-8',
  bom: false,
  lineEnding: 'lf',
  mixedLineEndings: false,
  readOnly: null,
});

beforeEach(() => useFileTabs.setState({ byConn: {} }));

describe('preview tabs', () => {
  it('replaces the previous preview in place', () => {
    store().open(CONN, '/a/one.txt', { preview: false });
    store().open(CONN, '/a/two.txt', { preview: true });
    store().open(CONN, '/a/three.txt', { preview: false });
    store().select(CONN, '/a/one.txt');
    store().open(CONN, '/a/four.txt', { preview: true });
    expect(tabs()).toEqual(['one.txt', 'four.txt*', 'three.txt']);
    expect(getFileTabs(CONN).activeKey).toBe('/a/four.txt');
  });

  it('opens a pinned tab after the active one', () => {
    store().open(CONN, '/a/one.txt', { preview: false });
    store().open(CONN, '/a/two.txt', { preview: false });
    store().select(CONN, '/a/one.txt');
    store().open(CONN, '/a/three.txt', { preview: false });
    expect(tabs()).toEqual(['one.txt', 'three.txt', 'two.txt']);
  });

  it('pins a preview when it is opened for good (Enter, double click)', () => {
    const first = store().open(CONN, '/a/one.txt', { preview: true });
    expect(first.load).toBe(true);
    const again = store().open(CONN, '/a/one.txt', { preview: false });
    expect(again).toEqual({ key: '/a/one.txt', load: false });
    expect(tabs()).toEqual(['one.txt']);
    // The next preview opens a new tab instead of replacing it.
    store().open(CONN, '/a/two.txt', { preview: true });
    expect(tabs()).toEqual(['one.txt', 'two.txt*']);
  });

  it('keeps a pinned tab pinned when previewed again', () => {
    store().open(CONN, '/a/one.txt', { preview: false });
    store().open(CONN, '/a/one.txt', { preview: true });
    expect(tabs()).toEqual(['one.txt']);
  });

  it('pins on "Keep open"', () => {
    store().open(CONN, '/a/one.txt', { preview: true });
    store().pin(CONN, '/a/one.txt');
    store().open(CONN, '/a/two.txt', { preview: true });
    expect(tabs()).toEqual(['one.txt', 'two.txt*']);
  });

  it('asks before loading files above the threshold', () => {
    const result = store().open(CONN, '/logs/app.log', {
      preview: true,
      knownSize: 200 * 1024 * 1024,
      needsConfirm: true,
    });
    expect(result.load).toBe(false);
    expect(getFileTabs(CONN).tabs[0]).toMatchObject({
      status: 'confirm',
      knownSize: 200 * 1024 * 1024,
    });
  });

  it('retries a failed tab when it is opened again', () => {
    store().open(CONN, '/a/one.txt', { preview: true });
    store().update(CONN, '/a/one.txt', { status: 'error' });
    expect(store().open(CONN, '/a/one.txt', { preview: true }).load).toBe(true);
  });
});

describe('closing', () => {
  it('activates the tab to the right, else the one to the left', () => {
    for (const name of ['one', 'two', 'three'])
      store().open(CONN, `/a/${name}`, { preview: false });
    store().select(CONN, '/a/two');
    store().close(CONN, '/a/two');
    expect(getFileTabs(CONN).activeKey).toBe('/a/three');
    store().close(CONN, '/a/three');
    expect(getFileTabs(CONN).activeKey).toBe('/a/one');
    store().close(CONN, '/a/one');
    expect(getFileTabs(CONN)).toEqual({ tabs: [], activeKey: null });
  });

  it('keeps the active tab when another one closes', () => {
    store().open(CONN, '/a/one', { preview: false });
    store().open(CONN, '/a/two', { preview: false });
    store().close(CONN, '/a/one');
    expect(getFileTabs(CONN).activeKey).toBe('/a/two');
  });

  it('reports dirty tabs per connection and for the window', () => {
    store().open(CONN, '/a/one', { preview: false });
    store().open('conn-2', '/b/two', { preview: false });
    store().update(CONN, '/a/one', { dirty: true });
    store().update('conn-2', '/b/two', { dirty: true });
    expect(dirtyFileTabs(CONN).map((tab) => tab.path)).toEqual(['/a/one']);
    expect(dirtyFileTabs()).toHaveLength(2);
    store().closeAll(CONN);
    expect(dirtyFileTabs()).toHaveLength(1);
  });
});

describe('loading', () => {
  it('stores the metadata, bumps the document version and detects the language', () => {
    store().open(CONN, '/srv/scripts/migrate', { preview: true });
    store().loaded(
      CONN,
      '/srv/scripts/migrate',
      opened('/srv/scripts/migrate', '#!/usr/bin/env python3\nprint(1)\n'),
      false,
    );
    const tab = getFileTabs(CONN).tabs[0]!;
    expect(tab).toMatchObject({ status: 'ready', docVersion: 1, language: 'python', dirty: false });
    expect(tab.meta).not.toHaveProperty('text');
    expect(tab.initialText).toContain('print(1)');
  });

  it('keeps a language chosen by hand across reloads', () => {
    store().open(CONN, '/srv/notes.txt', { preview: false });
    store().update(CONN, '/srv/notes.txt', { language: 'markdown', languageManual: true });
    store().loaded(CONN, '/srv/notes.txt', opened('/srv/notes.txt'), true);
    expect(getFileTabs(CONN).tabs[0]).toMatchObject({ language: 'markdown', whole: true });
  });
});
