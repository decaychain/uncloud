//! Retry policy for network storage backends.
//!
//! The local backend doesn't need this — the kernel retries transient I/O
//! at the VFS layer — but S3 and SFTP have to handle their own transient
//! failures (connection resets, throttling, mid-flight timeouts).
//!
//! `RetryConfig` is the user-facing knob (one block in `config.yaml`,
//! shared by both network backends). `retry()` is the helper that drives
//! the actual loop with exponential backoff + jitter.
//!
//! Callers are responsible for passing only **idempotent** closures —
//! reads, stats, lists, and write-to-temp-then-rename mutations are safe;
//! consume-an-input-stream uploads are not (the input reader is consumed
//! by the first attempt and can't be replayed).

use std::time::Duration;

use rand::Rng;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RetryConfig {
    /// Total tries including the first. `1` disables retry; `0` is treated
    /// as `1` so a misconfiguration can't silently swallow every error.
    pub max_attempts: u32,
    /// Initial backoff between attempts in milliseconds. Doubles every
    /// retry, capped at `max_delay_ms`. Jittered ±25 % per sleep so a
    /// thundering herd of retries doesn't synchronise.
    pub base_delay_ms: u64,
    /// Upper bound on the backoff between attempts.
    pub max_delay_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 200,
            max_delay_ms: 5_000,
        }
    }
}

impl RetryConfig {
    pub fn base_delay(&self) -> Duration {
        Duration::from_millis(self.base_delay_ms)
    }

    pub fn max_delay(&self) -> Duration {
        Duration::from_millis(self.max_delay_ms)
    }

    pub fn effective_max_attempts(&self) -> u32 {
        self.max_attempts.max(1)
    }
}

/// Run `f` up to `policy.max_attempts` times, sleeping with exponential
/// backoff between failures. Every error is treated as retryable — the
/// caller decides retry-eligibility by what it wraps.
///
/// `op_name` is logged on each retry so timing-related failures are
/// traceable in production logs without enabling debug noise everywhere.
pub async fn retry<F, Fut, T, E>(policy: &RetryConfig, op_name: &str, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let max = policy.effective_max_attempts();
    let mut delay = policy.base_delay();
    let max_delay = policy.max_delay();

    for attempt in 1..=max {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) if attempt < max => {
                let jittered = jitter(delay);
                tracing::warn!(
                    "storage {op_name}: attempt {attempt}/{max} failed: {e}; \
                     retrying in {:?}",
                    jittered,
                );
                tokio::time::sleep(jittered).await;
                delay = (delay * 2).min(max_delay);
            }
            Err(e) => return Err(e),
        }
    }
    unreachable!("loop returns from every iteration")
}

fn jitter(d: Duration) -> Duration {
    // ±25 %; a small linearly-distributed jitter is plenty to spread out a
    // synchronised retry storm and doesn't need a stronger PRNG.
    let ms = d.as_millis() as u64;
    let spread = ms / 4;
    let offset = if spread == 0 {
        0
    } else {
        rand::thread_rng().gen_range(0..=spread * 2) as i64 - spread as i64
    };
    Duration::from_millis(((ms as i64) + offset).max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn retries_then_succeeds() {
        let policy = RetryConfig {
            max_attempts: 3,
            base_delay_ms: 1,
            max_delay_ms: 4,
        };
        let attempts = AtomicU32::new(0);
        let result: Result<u32, &'static str> = retry(&policy, "test", || {
            let n = attempts.fetch_add(1, Ordering::Relaxed) + 1;
            async move {
                if n < 3 {
                    Err("transient")
                } else {
                    Ok(n)
                }
            }
        })
        .await;
        assert_eq!(result.unwrap(), 3);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts() {
        let policy = RetryConfig {
            max_attempts: 2,
            base_delay_ms: 1,
            max_delay_ms: 1,
        };
        let attempts = AtomicU32::new(0);
        let result: Result<(), &'static str> = retry(&policy, "test", || {
            attempts.fetch_add(1, Ordering::Relaxed);
            async move { Err("boom") }
        })
        .await;
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn zero_max_attempts_treated_as_one() {
        let policy = RetryConfig {
            max_attempts: 0,
            base_delay_ms: 1,
            max_delay_ms: 1,
        };
        let attempts = AtomicU32::new(0);
        let _: Result<(), &'static str> = retry(&policy, "test", || {
            attempts.fetch_add(1, Ordering::Relaxed);
            async move { Err("boom") }
        })
        .await;
        assert_eq!(attempts.load(Ordering::Relaxed), 1);
    }
}
