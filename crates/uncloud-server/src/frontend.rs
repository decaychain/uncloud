use axum::response::{IntoResponse, Response};
use axum::http::{header, StatusCode};
use axum::extract::Path;
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../../target/dx/uncloud-web/release/web/public/"]
struct Assets;

pub async fn static_handler(Path(path): Path<String>) -> Response {
    serve_embedded(&path)
}

pub async fn index_handler() -> Response {
    serve_embedded("index.html")
}

fn serve_embedded(path: &str) -> Response {
    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            (
                StatusCode::OK,
                [
                    (header::CONTENT_TYPE, mime),
                    (header::CACHE_CONTROL, cache_control(path).to_string()),
                ],
                file.data,
            )
                .into_response()
        }
        // SPA fallback: serve index.html for unmatched routes
        None => match Assets::get("index.html") {
            Some(file) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html".to_string()),
                 (header::CACHE_CONTROL, "no-cache".to_string())],
                file.data,
            )
                .into_response(),
            None => (StatusCode::NOT_FOUND, "frontend not embedded").into_response(),
        },
    }
}

fn cache_control(path: &str) -> &'static str {
    // Hashed asset filenames are immutable
    if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}
