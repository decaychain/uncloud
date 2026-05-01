//! Shorthand expanders for repository URIs.
//!
//! `rustic_backend` natively understands four schemes: `local`, `rclone`,
//! `rest`, `opendal`. Anything else has to go through `opendal:<scheme>`
//! with credentials in the BackendOptions map. That's friendly for advanced
//! users but punishing for "I just want to point at S3", so we expand a few
//! common shorthands here:
//!
//! - `s3:<http(s)://host[:port]/bucket[/prefix]>`
//! - `s3:<bucket>[/prefix]` (uses AWS S3 default endpoint)
//! - `b2:<bucket>[:prefix]`
//! - `azure:<container>[:prefix]`
//!
//! Each expansion returns the canonical `opendal:<service>` URI plus a map
//! of options that should be merged into the user's `credentials` block
//! before being passed to rustic. User-supplied options always win.

use std::collections::BTreeMap;

/// Result of expanding a shorthand URI: a (possibly-rewritten) URI for
/// rustic, and a set of synthesised options to merge in (overridable).
pub struct ExpandedUri {
    pub uri: String,
    pub options: BTreeMap<String, String>,
}

pub fn expand(input: &str) -> ExpandedUri {
    let Some((scheme, rest)) = input.split_once(':') else {
        // No scheme — local path.
        return ExpandedUri { uri: input.to_string(), options: BTreeMap::new() };
    };
    match scheme.to_ascii_lowercase().as_str() {
        // Native rustic_backend schemes pass through.
        "local" | "rclone" | "rest" | "opendal" => ExpandedUri {
            uri: input.to_string(),
            options: BTreeMap::new(),
        },
        "s3" => expand_s3(rest),
        "b2" => expand_b2(rest),
        "azure" => expand_azure(rest),
        // Unknown schemes pass through as-is — let rustic error if it's
        // not one it can handle.
        _ => ExpandedUri {
            uri: input.to_string(),
            options: BTreeMap::new(),
        },
    }
}

fn expand_s3(rest: &str) -> ExpandedUri {
    let mut options = BTreeMap::new();
    // OpenDAL's S3 service refuses to start without a region. MinIO and
    // most S3-compatible services don't care which value, but we have to
    // supply one. `us-east-1` is the canonical default that real AWS S3
    // also accepts as a valid bucket region. User can override via the
    // target's `credentials:` block.
    options.insert("region".to_string(), "us-east-1".to_string());
    let (endpoint, location) = if let Some(loc) = rest.strip_prefix("http://") {
        ("http://", loc)
    } else if let Some(loc) = rest.strip_prefix("https://") {
        ("https://", loc)
    } else {
        // Plain `s3:bucket[/prefix]` — AWS S3 default endpoint, no override.
        let (bucket, root) = split_bucket_root(rest);
        options.insert("bucket".to_string(), bucket.to_string());
        if let Some(r) = root {
            options.insert("root".to_string(), format!("/{r}"));
        }
        return ExpandedUri {
            uri: "opendal:s3".to_string(),
            options,
        };
    };
    // location now looks like `host[:port]/bucket[/prefix]`.
    let (host_port, path) = match location.split_once('/') {
        Some((h, p)) => (h, p),
        None => (location, ""),
    };
    options.insert(
        "endpoint".to_string(),
        format!("{endpoint}{host_port}"),
    );
    let (bucket, root) = split_bucket_root(path);
    if !bucket.is_empty() {
        options.insert("bucket".to_string(), bucket.to_string());
    }
    if let Some(r) = root {
        options.insert("root".to_string(), format!("/{r}"));
    }
    ExpandedUri {
        uri: "opendal:s3".to_string(),
        options,
    }
}

fn expand_b2(rest: &str) -> ExpandedUri {
    // `b2:bucket[:prefix]` — restic's classic shorthand.
    let mut options = BTreeMap::new();
    let (bucket, prefix) = match rest.split_once(':') {
        Some((b, p)) => (b, Some(p)),
        None => (rest, None),
    };
    options.insert("bucket".to_string(), bucket.to_string());
    if let Some(p) = prefix {
        options.insert("root".to_string(), format!("/{p}"));
    }
    ExpandedUri {
        uri: "opendal:b2".to_string(),
        options,
    }
}

fn expand_azure(rest: &str) -> ExpandedUri {
    // `azure:container[:prefix]` — opendal calls this `azblob`.
    let mut options = BTreeMap::new();
    let (container, prefix) = match rest.split_once(':') {
        Some((c, p)) => (c, Some(p)),
        None => (rest, None),
    };
    options.insert("container".to_string(), container.to_string());
    if let Some(p) = prefix {
        options.insert("root".to_string(), format!("/{p}"));
    }
    ExpandedUri {
        uri: "opendal:azblob".to_string(),
        options,
    }
}

fn split_bucket_root(s: &str) -> (&str, Option<&str>) {
    match s.split_once('/') {
        Some((b, r)) if !r.is_empty() => (b, Some(r)),
        _ => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_with_endpoint_and_bucket() {
        let r = expand("s3:http://127.0.0.1:9000/uncloud-backup");
        assert_eq!(r.uri, "opendal:s3");
        assert_eq!(r.options.get("endpoint").unwrap(), "http://127.0.0.1:9000");
        assert_eq!(r.options.get("bucket").unwrap(), "uncloud-backup");
        assert!(r.options.get("root").is_none());
    }

    #[test]
    fn s3_with_endpoint_bucket_and_prefix() {
        let r = expand("s3:https://s3.amazonaws.com/my-bucket/sub/dir");
        assert_eq!(r.options.get("endpoint").unwrap(), "https://s3.amazonaws.com");
        assert_eq!(r.options.get("bucket").unwrap(), "my-bucket");
        assert_eq!(r.options.get("root").unwrap(), "/sub/dir");
    }

    #[test]
    fn s3_bare_bucket() {
        let r = expand("s3:my-bucket/sub");
        assert_eq!(r.uri, "opendal:s3");
        assert_eq!(r.options.get("bucket").unwrap(), "my-bucket");
        assert_eq!(r.options.get("root").unwrap(), "/sub");
    }

    #[test]
    fn b2_with_prefix() {
        let r = expand("b2:my-bucket:nested/path");
        assert_eq!(r.uri, "opendal:b2");
        assert_eq!(r.options.get("bucket").unwrap(), "my-bucket");
        assert_eq!(r.options.get("root").unwrap(), "/nested/path");
    }

    #[test]
    fn passthrough_unknown_schemes() {
        for input in ["local:/tmp/repo", "/tmp/repo", "rest:http://x", "opendal:s3", "rclone:remote:path"] {
            let r = expand(input);
            assert_eq!(r.uri, input);
            assert!(r.options.is_empty(), "expected no synthesised options for {input}");
        }
    }
}
