/** Lucide icons for remote entries: folders, links, and a few file types. */
import {
  File,
  FileArchive,
  FileCode,
  FileCog,
  FileImage,
  FileJson,
  FileLock,
  FileTerminal,
  FileText,
  Folder,
  FolderSymlink,
  Link,
  ScrollText,
} from 'lucide-react';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import { detectLanguage } from '@/lib/languages';

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|ico|svg|avif|tiff?)$/i;
const ARCHIVE = /\.(zip|tar|gz|tgz|bz2|xz|zst|7z|rar|jar|war|deb|rpm)$/i;
const KEY = /\.(pem|key|crt|cer|p12|pfx|pub|gpg|asc)$|^id_(rsa|ed25519|ecdsa)/i;

export function FileIcon({ name, size = 14 }: { name: string; size?: number }) {
  const props = { size, 'aria-hidden': true } as const;
  if (IMAGE.test(name)) return <FileImage {...props} />;
  if (ARCHIVE.test(name)) return <FileArchive {...props} />;
  if (KEY.test(name)) return <FileLock {...props} />;
  switch (detectLanguage(name)) {
    case 'json':
      return <FileJson {...props} />;
    case 'shell':
    case 'makefile':
    case 'dockerfile':
      return <FileTerminal {...props} />;
    case 'ini':
    case 'properties':
    case 'dotenv':
    case 'toml':
    case 'yaml':
    case 'nginx':
    case 'apache':
      return <FileCog {...props} />;
    case 'log':
      return <ScrollText {...props} />;
    case 'markdown':
    case 'text':
      return name.includes('.') || name.startsWith('.') ? (
        <FileText {...props} />
      ) : (
        <File {...props} />
      );
    default:
      return <FileCode {...props} />;
  }
}

export function EntryIcon({ entry }: { entry: Entry }) {
  if (entry.kind === 'dir') return <Folder size={14} aria-hidden />;
  if (entry.kind === 'symlink') {
    return entry.targetKind === 'dir' ? (
      <FolderSymlink size={14} aria-hidden />
    ) : (
      <Link size={14} aria-hidden />
    );
  }
  return <FileIcon name={entry.name} />;
}
