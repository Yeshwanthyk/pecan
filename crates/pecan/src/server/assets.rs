//! Embedded web assets served at `/`.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// Files under the repo-root `web/` directory.
///
/// Debug builds read from disk live (fast iteration); release builds embed
/// them into the binary.
#[derive(RustEmbed)]
#[folder = "../../web/dist"]
struct Assets;

/// Serves one asset by path, falling back to `index.html` for SPA routes.
pub(crate) async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    // Hashed asset filenames are content-addressed (immutable); the shell HTML
    // must always revalidate so deploys are picked up.
    if let Some(file) = Assets::get(path) {
        let mime = mime_for(path);
        let cache = if path.starts_with("assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        return ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, cache)], file.data)
            .into_response();
    }
    match Assets::get("index.html") {
        Some(index) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            index.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "web assets missing").into_response(),
    }
}

fn mime_for(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("webmanifest") => "application/manifest+json",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webmanifest_gets_manifest_mime() {
        assert_eq!(mime_for("manifest.webmanifest"), "application/manifest+json");
        assert_eq!(mime_for("foo.json"), "application/json");
    }
}
