// Route matching: method + path segments with :param capture.
use crate::config::Route;
use std::collections::HashMap;

pub struct Match {
    pub route_index: usize,
    pub params: HashMap<String, String>,
}

/// Return the first route that handles this method and path. Declaration order wins.
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
            // tradeoff: path parameters stay percent-encoded until transform slice
            params.insert(name.to_string(), (*actual).to_string());
        } else if expected != actual {
            return None;
        }
    }
    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;

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
