use super::*;
use talocart_platform::media::{ImageVariant, MediaImage};
#[derive(serde::Deserialize, Default)]
pub struct ImageQuery {
    #[serde(default)]
    pub variant: ImageVariant,
}
pub fn fallback() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/svg+xml"),
            (header::CACHE_CONTROL, "no-store"),
            (
                header::HeaderName::from_static("x-robots-tag"),
                "noindex, nofollow",
            ),
        ],
        include_bytes!("../../../assets/favicon.svg").as_slice(),
    )
        .into_response()
}
pub fn response(image: MediaImage, headers: &HeaderMap, public: bool) -> Response {
    let etag = format!("\"media-{}\"", image.metadata.id);
    let cached = matches_etag(headers, &etag);
    let mut response = if cached {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        image.png.into_response()
    };
    let out = response.headers_mut();
    let mime = match image.metadata.content_type.as_str() {
        "image/jpeg" => "image/jpeg",
        "image/webp" => "image/webp",
        _ => "image/png",
    };
    out.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime));
    out.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if public {
            // Publication can change while the image URL and ETag remain stable.
            "public, max-age=0, must-revalidate"
        } else {
            "private, no-cache"
        }),
    );
    if let Ok(value) = HeaderValue::from_str(&etag) {
        out.insert(header::ETAG, value);
    }
    response
}

/// Both logical profile URLs and exact object URLs share validators and HEAD
/// behavior. The caller resolves current public access before reaching this point.
pub async fn public_file_response(
    state: &AppState,
    file: talocart_platform::media::ImageFile,
    headers: &HeaderMap,
    method: axum::http::Method,
) -> Result<Response, talocart_platform::media::MediaError> {
    let cached = matches_etag(headers, &format!("\"media-{}\"", file.id));
    let size = file.byte_size;
    let bytes = if cached || method == axum::http::Method::HEAD {
        Vec::new()
    } else {
        state.media.read_registered(&state.pool, &file.key).await?
    };
    let mut response = response(
        MediaImage {
            metadata: file.metadata(),
            png: bytes,
        },
        headers,
        true,
    );
    if method == axum::http::Method::HEAD && !cached {
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    }
    Ok(response)
}

pub fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(','))
        .any(|v| {
            let v = v.trim();
            v == "*" || v.strip_prefix("W/").unwrap_or(v) == etag
        })
}
