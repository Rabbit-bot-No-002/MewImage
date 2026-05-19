fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|item| item == &value) {
        values.push(value);
    }
}

fn normalized_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn local_backend_candidates(path: &str, scheme: &str, hostname: &str) -> Vec<String> {
    let mut values = Vec::new();
    let normalized = normalized_path(path);
    if !hostname.is_empty() {
        push_unique(
            &mut values,
            format!("{scheme}://{hostname}:3000{normalized}"),
        );
    }
    push_unique(
        &mut values,
        format!("{scheme}://127.0.0.1:3000{normalized}"),
    );
    push_unique(
        &mut values,
        format!("{scheme}://localhost:3000{normalized}"),
    );
    values
}

pub fn api_candidates(path: &str) -> Vec<String> {
    let normalized = normalized_path(path);
    let mut values = Vec::new();
    let Some(window) = web_sys::window() else {
        return local_backend_candidates(&normalized, "http", "127.0.0.1");
    };
    let location = window.location();
    let protocol = location.protocol().unwrap_or_default();
    let hostname = location.hostname().unwrap_or_default();
    let port = location.port().unwrap_or_default();
    let is_http = protocol == "http:" || protocol == "https:";
    let scheme = if protocol == "https:" {
        "https"
    } else {
        "http"
    };
    let is_local_host = matches!(hostname.as_str(), "localhost" | "127.0.0.1");

    if is_http {
        if is_local_host && port != "3000" {
            values.extend(local_backend_candidates(&normalized, scheme, &hostname));
            if let Ok(origin) = location.origin() {
                if !origin.is_empty() && origin != "null" {
                    push_unique(&mut values, format!("{origin}{normalized}"));
                }
            }
        } else if let Ok(origin) = location.origin() {
            if !origin.is_empty() && origin != "null" {
                push_unique(&mut values, format!("{origin}{normalized}"));
            }
            if is_local_host {
                values.extend(local_backend_candidates(&normalized, scheme, &hostname));
            }
        }
        push_unique(&mut values, normalized);
        return values;
    }

    local_backend_candidates(&normalized, "http", "127.0.0.1")
}

pub fn api_url(path: &str) -> String {
    api_candidates(path)
        .into_iter()
        .next()
        .unwrap_or_else(|| normalized_path(path))
}
