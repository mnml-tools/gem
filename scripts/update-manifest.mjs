#!/usr/bin/env node
// Writes latest.json, the manifest the in-app updater reads from the latest GitHub Release
// (ADR 0015). Every package that Tauri signed for the updater gets its own
// `{os}-{arch}-{bundle}` entry, so a .deb install updates with the .deb, an MSI install with
// the MSI, and so on; the plain `{os}-{arch}` entry is the fallback (AppImage, app, NSIS).
//
// Usage: node scripts/update-manifest.mjs <artifacts dir> <owner/repo> <tag>
import { readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');

/** Updater packages: file suffix → manifest targets. */
const PACKAGES = [
  ['.AppImage', ['linux-x86_64-appimage', 'linux-x86_64']],
  ['.deb', ['linux-x86_64-deb']],
  ['.rpm', ['linux-x86_64-rpm']],
  // The universal app serves both Mac architectures.
  ['.app.tar.gz', ['darwin-aarch64-app', 'darwin-x86_64-app', 'darwin-aarch64', 'darwin-x86_64']],
  ['-setup.exe', ['windows-x86_64-nsis', 'windows-x86_64']],
  ['.msi', ['windows-x86_64-msi']],
];

export function createManifest(dir, repository, tag, version, notes) {
  if (tag !== `v${version}`)
    throw new Error(`tag ${tag} does not match the app version ${version}`);
  const files = readdirSync(dir);
  const platforms = {};
  for (const [suffix, targets] of PACKAGES) {
    const matches = files.filter((file) => file.endsWith(suffix));
    if (matches.length !== 1) {
      throw new Error(`expected one ${suffix} package, found ${matches.length}`);
    }
    const file = matches[0];
    const signature = readFileSync(join(dir, `${file}.sig`), 'utf8').trim();
    if (!signature) throw new Error(`empty updater signature for ${file}`);
    const url = `https://github.com/${repository}/releases/download/${encodeURIComponent(tag)}/${encodeURIComponent(file)}`;
    for (const target of targets) platforms[target] = { url, signature };
  }
  return { version, notes, pub_date: new Date().toISOString(), platforms };
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const [dir, repository, tag] = process.argv.slice(2);
  if (!dir || !repository || !tag) {
    console.error('usage: node scripts/update-manifest.mjs <artifacts dir> <owner/repo> <tag>');
    process.exit(2);
  }
  const { version } = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf8'));
  const notes = `See https://github.com/${repository}/releases/tag/${tag}`;
  const manifest = createManifest(dir, repository, tag, version, notes);
  writeFileSync(join(dir, 'latest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
  console.log(`latest.json: ${Object.keys(manifest.platforms).join(', ')}`);
}
