use std::{future::Future, pin::Pin, time::Duration};

use rand::RngExt;

#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub jitter_frac: f64,
}

pub async fn retry<T, F, Fut, C>(
    cfg: RetryConfig,
    is_transient: C,
    label: &str,
    op: F,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
    C: Fn(&anyhow::Error) -> bool,
{
    retry_inner(cfg, is_transient, label, op, |d| {
        Box::pin(tokio::time::sleep(d))
    })
    .await
}

async fn retry_inner<T, F, Fut, C, S>(
    cfg: RetryConfig,
    is_transient: C,
    label: &str,
    mut op: F,
    sleeper: S,
) -> anyhow::Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
    C: Fn(&anyhow::Error) -> bool,
    S: Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>,
{
    let max = cfg.max_attempts.max(1);
    let mut attempt: u32 = 0;
    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(err) => {
                let last = attempt + 1 >= max;
                if last || !is_transient(&err) {
                    return Err(err);
                }
                let delay = backoff_delay(&cfg, attempt);
                log::warn!(
                    "{label} attempt {}/{} failed (transient): {err}; retrying in {:?}",
                    attempt + 1,
                    max,
                    delay,
                );
                sleeper(delay).await;
                attempt += 1;
            }
        }
    }
}

fn backoff_delay(cfg: &RetryConfig, attempt: u32) -> Duration {
    let base_ms = cfg.base_delay.as_millis() as f64;
    let max_ms = cfg.max_delay.as_millis() as f64;
    let exp = base_ms * 2f64.powi(attempt as i32);
    let capped = exp.min(max_ms);

    let frac = cfg.jitter_frac.clamp(0.0, 1.0);
    let jitter = rand::rng().random_range(-frac..=frac);
    let jittered = (capped * (1.0 + jitter)).max(1.0);

    Duration::from_millis(jittered as u64)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    fn cfg(max_attempts: u32) -> RetryConfig {
        RetryConfig {
            max_attempts,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
            jitter_frac: 0.0,
        }
    }

    #[allow(clippy::type_complexity)]
    fn recording_sleeper() -> (
        Arc<Mutex<Vec<Duration>>>,
        impl Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>,
    ) {
        let log = Arc::new(Mutex::new(Vec::<Duration>::new()));
        let sink = log.clone();
        let sleeper = move |d: Duration| -> Pin<Box<dyn Future<Output = ()> + Send>> {
            sink.lock().unwrap().push(d);
            Box::pin(async {})
        };
        (log, sleeper)
    }

    #[tokio::test]
    async fn first_attempt_success_no_sleeps() {
        let (log, sleeper) = recording_sleeper();
        let out: anyhow::Result<i32> =
            retry_inner(cfg(3), |_| true, "test", || async { Ok(42) }, sleeper).await;
        assert_eq!(out.unwrap(), 42);
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn two_transient_then_success_records_two_sleeps() {
        let (log, sleeper) = recording_sleeper();
        let calls = Arc::new(Mutex::new(0u32));
        let calls_clone = calls.clone();
        let out: anyhow::Result<i32> = retry_inner(
            cfg(3),
            |_| true,
            "test",
            move || {
                let calls = calls_clone.clone();
                async move {
                    let mut n = calls.lock().unwrap();
                    *n += 1;
                    if *n < 3 {
                        Err(anyhow::anyhow!("boom {}", *n))
                    } else {
                        Ok(7)
                    }
                }
            },
            sleeper,
        )
        .await;
        assert_eq!(out.unwrap(), 7);
        assert_eq!(*calls.lock().unwrap(), 3);
        let sleeps = log.lock().unwrap().clone();
        assert_eq!(sleeps.len(), 2);
        assert!(sleeps[1] >= sleeps[0], "exponential should be monotonic");
    }

    #[tokio::test]
    async fn non_transient_returns_immediately() {
        let (log, sleeper) = recording_sleeper();
        let calls = Arc::new(Mutex::new(0u32));
        let calls_clone = calls.clone();
        let out: anyhow::Result<i32> = retry_inner(
            cfg(5),
            |err| !err.to_string().contains("permanent"),
            "test",
            move || {
                let calls = calls_clone.clone();
                async move {
                    let mut n = calls.lock().unwrap();
                    *n += 1;
                    if *n == 1 {
                        Err(anyhow::anyhow!("transient"))
                    } else {
                        Err(anyhow::anyhow!("permanent"))
                    }
                }
            },
            sleeper,
        )
        .await;
        let err = out.unwrap_err();
        assert_eq!(err.to_string(), "permanent");
        assert_eq!(*calls.lock().unwrap(), 2);
        assert_eq!(log.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn exhausted_returns_last_error() {
        let (log, sleeper) = recording_sleeper();
        let calls = Arc::new(Mutex::new(0u32));
        let calls_clone = calls.clone();
        let out: anyhow::Result<i32> = retry_inner(
            cfg(3),
            |_| true,
            "test",
            move || {
                let calls = calls_clone.clone();
                async move {
                    let mut n = calls.lock().unwrap();
                    *n += 1;
                    Err(anyhow::anyhow!("err {}", *n))
                }
            },
            sleeper,
        )
        .await;
        assert_eq!(out.unwrap_err().to_string(), "err 3");
        assert_eq!(*calls.lock().unwrap(), 3);
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn max_attempts_one_no_sleeps_on_failure() {
        let (log, sleeper) = recording_sleeper();
        let out: anyhow::Result<i32> = retry_inner(
            cfg(1),
            |_| true,
            "test",
            || async { Err(anyhow::anyhow!("nope")) },
            sleeper,
        )
        .await;
        assert!(out.is_err());
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let cfg = RetryConfig {
            max_attempts: 5,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(10),
            jitter_frac: 0.25,
        };
        let nominal = 1000.0_f64;
        let lo = (nominal * 0.75) as u64;
        let hi = (nominal * 1.25) as u64;
        for _ in 0..200 {
            let d = backoff_delay(&cfg, 0).as_millis() as u64;
            assert!(d >= lo && d <= hi, "delay {d} out of [{lo}, {hi}]");
        }
    }

    #[test]
    fn backoff_is_capped_by_max_delay() {
        let cfg = RetryConfig {
            max_attempts: 20,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_millis(500),
            jitter_frac: 0.0,
        };
        let d = backoff_delay(&cfg, 10);
        assert_eq!(d, Duration::from_millis(500));
    }
}
