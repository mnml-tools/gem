/**
 * Window chrome per platform, as Piw's and Zap's: no system title bar. macOS keeps its traffic
 * lights over the title bar (`macInset`); Linux and Windows draw the kit's `WindowControls`;
 * Linux also needs resize edges, because an undecorated GTK window has none.
 */
import { isTauri } from '@tauri-apps/api/core';
import { detectPlatform } from './theme';

const platform = detectPlatform();
export const nativeWindow = isTauri();
export const drawsWindowControls = nativeWindow && platform !== 'macos';
export const drawsResizeEdges = nativeWindow && platform === 'linux';
export const macInset = nativeWindow && platform === 'macos';
