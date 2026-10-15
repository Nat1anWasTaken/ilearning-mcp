//! Static serving of the built dashboard SPA.

use std::path::Path;

use axum::{
    Router,
    extract::Request,
    http::{HeaderValue, header},
    middleware::{self, Next},
    response::Response,
};
use tower_http::services::{ServeDir, ServeFile};

/// Serves the built dashboard. Unknown paths get `index.html` with `200` so
/// client-side routes like `/sessions/123` resolve.
pub fn service(dist: &Path) -> Router {
    let files = ServeDir::new(dist).fallback(ServeFile::new(dist.join("index.html")));
    Router::new()
        .fallback_service(files)
        .layer(middleware::from_fn(cache_control))
}

/// Vite's hashed assets never change; everything else (`index.html`, the
/// service worker, the web app manifest) must be revalidated so a deploy
/// reaches installed apps.
async fn cache_control(request: Request, next: Next) -> Response {
    let immutable = request.uri().path().starts_with("/assets/");
    let mut response = next.run(request).await;
    let value = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    if response.status().is_success() {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(value));
    }
    response
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    use super::*;

    fn dist(name: &str) -> std::path::PathBuf {
        let dist = std::env::temp_dir().join(format!("web-dist-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<!doctype html>").unwrap();
        dist
    }

    #[tokio::test]
    async fn client_routes_get_the_index_with_200() {
        let dist = dist("routes");
        let response = service(&dist)
            .oneshot(Request::get("/sessions/123").body(Body::empty()).unwrap())
            .await
            .unwrap();
        std::fs::remove_dir_all(&dist).unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    }

    #[tokio::test]
    async fn the_manifest_is_served_as_one() {
        let dist = dist("manifest");
        std::fs::write(dist.join("manifest.webmanifest"), "{}").unwrap();
        let response = service(&dist)
            .oneshot(
                Request::get("/manifest.webmanifest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        std::fs::remove_dir_all(&dist).unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "application/manifest+json"
        );
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    }
}
