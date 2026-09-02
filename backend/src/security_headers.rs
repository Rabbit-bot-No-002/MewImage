use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
    response::Response,
};

const CONTENT_SECURITY_POLICY: &str = concat!(
    "default-src 'self'; ",
    "base-uri 'self'; object-src 'none'; frame-ancestors 'none'; ",
    "script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; ",
    "style-src 'self' 'unsafe-inline'; font-src 'self' data:; ",
    "img-src 'self' blob: data: https: http:; ",
    "connect-src 'self' blob: https: http: ws: wss:; ",
    "worker-src 'self' blob:"
);

pub async fn apply_security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // 仅在上游未设置时补默认值，允许部署者在反向代理处采用更严格的策略。
    insert_if_absent(headers, "content-security-policy", CONTENT_SECURITY_POLICY);
    insert_if_absent(headers, "x-content-type-options", "nosniff");
    insert_if_absent(headers, "referrer-policy", "no-referrer");
    insert_if_absent(
        headers,
        "permissions-policy",
        "camera=(), microphone=(), geolocation=(), payment=()",
    );
    insert_if_absent(headers, "x-frame-options", "DENY");
    response
}

fn insert_if_absent(headers: &mut HeaderMap, name: &'static str, value: &'static str) {
    headers
        .entry(HeaderName::from_static(name))
        .or_insert(HeaderValue::from_static(value));
}
