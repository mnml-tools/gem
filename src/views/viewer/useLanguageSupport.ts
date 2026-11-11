/** The CodeMirror extension of a language, loaded lazily once and shared by every view. */
import type { Extension } from '@codemirror/state';
import { useEffect, useState } from 'react';
import { loadLanguage } from '@/lib/languageSupport';
import type { LanguageId } from '@/lib/languages';

const loaded = new Map<LanguageId, Extension>();
const pending = new Map<LanguageId, Promise<Extension>>();

function load(id: LanguageId): Promise<Extension> {
  let promise = pending.get(id);
  if (!promise) {
    promise = loadLanguage(id).then((extension) => {
      loaded.set(id, extension);
      return extension;
    });
    pending.set(id, promise);
    // A failed chunk load can be retried later.
    promise.catch(() => pending.delete(id));
  }
  return promise;
}

export function useLanguageSupport(id: LanguageId | null): Extension | null {
  const [, setVersion] = useState(0);
  useEffect(() => {
    if (!id || loaded.has(id)) return;
    let alive = true;
    load(id)
      .then(() => alive && setVersion((value) => value + 1))
      .catch((error: unknown) => console.error('Unable to load language support:', error));
    return () => {
      alive = false;
    };
  }, [id]);
  return id ? (loaded.get(id) ?? null) : null;
}
