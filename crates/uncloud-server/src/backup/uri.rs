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
//! - `sftp://[user@]host[:port][/path]` — URL style, supports custom port
//! - `sftp:[user@]host:/path` — legacy restic-flavoured form, no port
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
        "sftp" => expand_sftp(rest),
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

/// Translate `sftp:` shorthand into `opendal:sftp` with the right
/// endpoint / user / root options. Two formats are accepted:
///
/// * `sftp://[user@]host[:port][/path]` — URL style. The `:port` is the
///   reason this expander exists at all; OpenDAL's SFTP service takes
///   the port as part of the `endpoint=ssh://host:port` value.
/// * `sftp:[user@]host:/path` — legacy restic-flavoured shorthand.
///   Always default port 22; included for backward compat with examples
///   from the upstream `restic` docs.
///
/// Auth is key-based — OpenDAL's SFTP service has no password field.
/// Users can supply `key: /path/to/private-key` (and optionally
/// `user:`, `known_hosts_strategy:`) in the target's `credentials:`
/// block; user-supplied options always win, matching the s3/b2 path.
fn expand_sftp(rest: &str) -> ExpandedUri {
    let mut options = BTreeMap::new();

    let (user, host_port, root) = if let Some(url_rest) = rest.strip_prefix("//") {
        // URL form: [user@]host[:port][/path]
        let (user, hostpart) = match url_rest.split_once('@') {
            Some((u, h)) => (Some(u.to_string()), h),
            None => (None, url_rest),
        };
        let (host_port, root) = match hostpart.split_once('/') {
            Some((hp, p)) => (hp.to_string(), format!("/{p}")),
            None => (hostpart.to_string(), String::new()),
        };
        (user, host_port, root)
    } else {
        // Legacy form: [user@]host:/path  (no port — `:` separates host
        // from path, not host from port)
        let (user, hostpart) = match rest.split_once('@') {
            Some((u, h)) => (Some(u.to_string()), h),
            None => (None, rest),
        };
        let (host, root) = match hostpart.split_once(":/") {
            Some((h, p)) => (h.to_string(), format!("/{p}")),
            None => (hostpart.to_string(), String::new()),
        };
        (user, host, root)
    };

    options.insert("endpoint".to_string(), format!("ssh://{host_port}"));
    if let Some(u) = user {
        options.insert("user".to_string(), u);
    }
    if !root.is_empty() {
        options.insert("root".to_string(), root);
    }

    ExpandedUri {
        uri: "opendal:sftp".to_string(),
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
    fn sftp_url_with_port() {
        let r = expand("sftp://backup@nas.lan:2222/srv/backups/uncloud");
        assert_eq!(r.uri, "opendal:sftp");
        assert_eq!(r.options.get("endpoint").unwrap(), "ssh://nas.lan:2222");
        assert_eq!(r.options.get("user").unwrap(), "backup");
        assert_eq!(r.options.get("root").unwrap(), "/srv/backups/uncloud");
    }

    #[test]
    fn sftp_url_no_user_no_port() {
        let r = expand("sftp://nas.lan/srv/backups/uncloud");
        assert_eq!(r.options.get("endpoint").unwrap(), "ssh://nas.lan");
        assert!(r.options.get("user").is_none());
        assert_eq!(r.options.get("root").unwrap(), "/srv/backups/uncloud");
    }

    #[test]
    fn sftp_legacy_shorthand() {
        let r = expand("sftp:backup@nas.lan:/srv/backups/uncloud");
        assert_eq!(r.uri, "opendal:sftp");
        assert_eq!(r.options.get("endpoint").unwrap(), "ssh://nas.lan");
        assert_eq!(r.options.get("user").unwrap(), "backup");
        assert_eq!(r.options.get("root").unwrap(), "/srv/backups/uncloud");
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
