use super::BackoffStrategy;
use std::time::Duration;
use tokio::time::sleep;

pub fn calculate_delay(strategy: &BackoffStrategy, attempt: u32, _timeout: Duration) -> Duration {
    match strategy {
        BackoffStrategy::Exponential { base, max } => {
            let delay = *base * 2u32.pow(attempt);
            if delay > *max {
                *max
            } else {
                delay
            }
        }
        BackoffStrategy::Linear { interval } => *interval * (attempt + 1),
        BackoffStrategy::Fixed { interval } => *interval,
    }
}

pub async fn with_retry<F, Fut, T, E>(
    strategy: &BackoffStrategy,
    max_retries: u32,
    operation: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut attempt = 0u32;
    loop {
        match operation().await {
            Ok(result) => return Ok(result),
            Err(e) => {
                attempt += 1;
                if attempt >= max_retries {
                    return Err(e);
                }
                let delay = calculate_delay(strategy, attempt, Duration::from_secs(0));
                sleep(delay).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let strategy = BackoffStrategy::Exponential {
            base: Duration::from_millis(100),
            max: Duration::from_secs(5),
        };

        assert_eq!(calculate_delay(&strategy, 0, Duration::ZERO), Duration::from_millis(100));
        assert_eq!(calculate_delay(&strategy, 1, Duration::ZERO), Duration::from_millis(200));
        assert_eq!(calculate_delay(&strategy, 2, Duration::ZERO), Duration::from_millis(400));
        assert!(calculate_delay(&strategy, 10, Duration::ZERO) <= Duration::from_secs(5));
    }

    #[test]
    fn test_linear_backoff() {
        let strategy = BackoffStrategy::Linear { interval: Duration::from_millis(50) };

        assert_eq!(calculate_delay(&strategy, 0, Duration::ZERO), Duration::from_millis(50));
        assert_eq!(calculate_delay(&strategy, 1, Duration::ZERO), Duration::from_millis(100));
        assert_eq!(calculate_delay(&strategy, 2, Duration::ZERO), Duration::from_millis(150));
    }

    #[tokio::test]
    async fn test_retry_success_first_try() {
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let strategy = BackoffStrategy::Fixed { interval: Duration::from_millis(10) };
        
        let result = with_retry(&strategy, 3, || {
            let count = &call_count;
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok::<i32, &str>(42)
            }
        }).await;
        
        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_success_after_failures() {
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let strategy = BackoffStrategy::Fixed { interval: Duration::from_millis(10) };
        
        let result = with_retry(&strategy, 3, || {
            let count = &call_count;
            async move {
                let prev = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if prev < 2 {
                    Err("not ready")
                } else {
                    Ok(42)
                }
            }
        }).await;
        
        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_retry_all_fail() {
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let strategy = BackoffStrategy::Fixed { interval: Duration::from_millis(10) };
        
        let result = with_retry(&strategy, 2, || {
            let count = &call_count;
            async move {
                count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err::<i32, &str>("always fails")
            }
        }).await;
        
        assert!(result.is_err());
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    }
}