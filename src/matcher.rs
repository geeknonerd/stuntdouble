// Route matching: method + path segments with :param capture.
use crate::config::Route;
use std::collections::HashMap;

pub struct Match {
    pub route_index: usize,
    pub params: HashMap<String, String>,
}

/// Return the first route that handles this method and path. Declaration order wins.
/// Must use result to avoid silent drop of matched routes.
#[must_use]
pub fn match_route(routes: &[Route], method: &str, path: &str) -> Option<Match> {
    let method = method.to_ascii_uppercase();
    let segments = split(path);
    for (index, route) in routes.iter().enumerate() {
        if route.method != method {
            continue;
        }
        if let Some(params) = match_segments(&route.path, &segments) {
            return Some(Match {
                route_index: index,
                params,
            });
        }
    }
    None
}

fn split(value: &str) -> Vec<&str> {
    value
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

fn match_segments(pattern: &str, segments: &[&str]) -> Option<HashMap<String, String>> {
    let pattern_segments = split(pattern);
    if pattern_segments.len() != segments.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (expected, actual) in pattern_segments.iter().zip(segments) {
        if let Some(name) = expected.strip_prefix(':') {
            if name.is_empty() {
                return None; // malformed param like ':'
            }
            params.insert(name.to_string(), percent_decode(actual.as_bytes()));
        } else if expected != actual {
            return None;
        }
    }
    Some(params)
}

/// Percent-decode one path segment or query token. Invalid escapes and
/// non-UTF-8 bytes are replaced rather than rejected, because a mock server
/// must answer instead of failing on a malformed client request.
///
/// Must use result to avoid silent failures.
#[must_use]
pub fn percent_decode(bytes: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_decode_handles_escapes_and_falls_back() {
        assert_eq!(percent_decode(b"a%20b"), "a b");
        assert_eq!(percent_decode(b"DOC%2D0001"), "DOC-0001");
        assert_eq!(percent_decode(b"plain"), "plain");
        assert_eq!(percent_decode(b"bad%zz%"), "bad%zz%");
        assert_eq!(percent_decode(b"tail%2"), "tail%2");
    }

    #[test]
    fn path_params_are_decoded() {
        let routes = vec![route("GET", "/x/:id")];
        let found = match_route(&routes, "GET", "/x/a%20b").unwrap();
        assert_eq!(found.params.get("id").map(String::as_str), Some("a b"));
    }

    fn route(method: &str, path: &str) -> Route {
        Route {
            name: None,
            method: method.into(),
            path: path.into(),
            script: std::path::PathBuf::from("s.js"),
        }
    }

    #[test]
    fn captures_named_segment() {
        let routes = vec![route("GET", "/demo/documents/manifest/:group")];
        let found = match_route(&routes, "GET", "/demo/documents/manifest/group-a").unwrap();
        assert_eq!(found.route_index, 0);
        assert_eq!(found.params["group"], "group-a");
    }

    #[test]
    fn method_is_compared_after_uppercasing() {
        let routes = vec![route("GET", "/x")];
        assert!(match_route(&routes, "get", "/x").is_some());
        assert!(match_route(&routes, "POST", "/x").is_none());
    }

    #[test]
    fn segment_count_must_match() {
        let routes = vec![route("GET", "/x/:id")];
        assert!(match_route(&routes, "GET", "/x").is_none());
        assert!(match_route(&routes, "GET", "/x/1/2").is_none());
    }

    #[test]
    fn first_declaration_wins() {
        let routes = vec![route("GET", "/x/:id"), route("GET", "/x/:other")];
        let found = match_route(&routes, "GET", "/x/7").unwrap();
        assert_eq!(found.route_index, 0);
        assert!(found.params.contains_key("id"));
    }

    #[test]
    fn root_path_matches_root_route() {
        let routes = vec![route("GET", "/")];
        assert!(match_route(&routes, "GET", "/").is_some());
        assert!(match_route(&routes, "GET", "").is_some());
    }
}
