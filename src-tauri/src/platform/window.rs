//! Creation of the main window.

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::error::AppResult;

pub const MAIN: &str = "main";

/// Returns the main window, creating it if needed. Its navigation is locked to the app's own
/// pages (see [`super::navigation`]) and new windows are denied.
pub fn ensure_main<R: Runtime>(app: &AppHandle<R>) -> AppResult<WebviewWindow<R>> {
    if let Some(window) = app.get_webview_window(MAIN) {
        return Ok(window);
    }
    let dev_server = if cfg!(debug_assertions) {
        app.config().build.dev_url.clone()
    } else {
        None
    };
    let builder = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Gem")
        .inner_size(1180.0, 760.0)
        .min_inner_size(720.0, 480.0)
        .center()
        .on_navigation(move |url| {
            let allowed = super::navigation::is_app_url(url, dev_server.as_ref());
            if !allowed {
                log::warn!("blocked navigation away from the app");
            }
            allowed
        })
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny);
    // The app's title bar is drawn in the webview (docs/ui-design-system.md). macOS keeps its
    // native traffic lights, overlaid on the bar; Linux and Windows draw their own window
    // controls and resize edges.
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(12.0, 18.0));
    #[cfg(not(target_os = "macos"))]
    let builder = builder.decorations(false);
    Ok(builder.build()?)
}

/// Brings the main window to the front (second launch).
pub fn show_main<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let window = ensure_main(app)?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    Ok(())
}
