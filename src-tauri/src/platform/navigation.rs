//! Navigation lockdown. The Gem webview may only show Gem's own pages: any other navigation
//! (a link in pasted content, a compromised renderer) is refused, and new windows are denied.

use tauri::Url;

/// Whether `url` is one of the app's own pages (bundled assets, or the dev server in debug).
pub fn is_app_url(url: &Url, dev_server: Option<&Url>) -> bool {
    match url.scheme() {
        // Bundled assets: `tauri://localhost` (Linux/macOS), `http(s)://tauri.localhost` (Windows).
        "tauri" => url.host_str() == Some("localhost"),
        "http" | "https" => {
            url.host_str() == Some("tauri.localhost")
                || dev_server.is_some_and(|dev| {
                    dev.scheme() == url.scheme()
                        && dev.host_str() == url.host_str()
                        && dev.port_or_known_default() == url.port_or_known_default()
                })
        }
        // `about:blank` is used internally by webviews while loading.
        "about" => url.as_str() == "about:blank",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn only_app_pages_are_allowed() {
        assert!(is_app_url(&url("tauri://localhost/index.html"), None));
        assert!(is_app_url(&url("http://tauri.localhost/index.html"), None));
        assert!(is_app_url(&url("about:blank"), None));
        for bad in [
            "https://example.com/",
            "http://tauri.localhost.evil.com/",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "http://localhost:1426/",
            "tauri://evil/",
        ] {
            assert!(!is_app_url(&url(bad), None), "{bad}");
        }
    }

    #[test]
    fn the_dev_server_is_allowed_only_when_given() {
        let dev = url("http://localhost:1426/");
        assert!(is_app_url(
            &url("http://localhost:1426/src/main.tsx"),
            Some(&dev)
        ));
        assert!(!is_app_url(&url("http://localhost:1427/"), Some(&dev)));
    }
}
