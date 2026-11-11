/**
 * Stand-in for the optional peers of `@mnml-tools/ui/tauri` (`@tauri-apps/plugin-process`,
 * `@tauri-apps/plugin-updater`), which only its `UpdateDialog` / `useAppUpdate` use. Gem has no
 * updater in the MVP (plan §10.2), so it does not install those plugins; `vite.config.ts`
 * aliases both imports here so the kit's `WindowControls` and `syncNativeTheme` can be used.
 * Nothing in Gem calls these: they fail loudly if something ever does.
 */
const unavailable = (name: string) => () =>
  Promise.reject(new Error(`${name} is not available in Gem (no updater in the MVP)`));

export const check = unavailable('@tauri-apps/plugin-updater');
export const relaunch = unavailable('@tauri-apps/plugin-process');
