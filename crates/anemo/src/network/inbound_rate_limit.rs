use std::collections::HashMap;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::time::Instant;

/// Hard cap on the number of distinct source IPs tracked at once.
/// At a few dozen bytes per entry this bounds the map near ~2 MiB.
const MAX_TRACKED_IPS: usize = 16_384;

/// A token-bucket rate limiter keyed by source IP, used to bound the rate at which
/// new inbound connections from any single source are admitted to the TLS handshake.
pub(crate) struct InboundIpRateLimiter {
    /// Sustained refill rate, in tokens (connections) per second.
    rate_per_sec: f64,
    /// Bucket capacity, i.e. the largest instantaneous burst.
    burst: f64,
    buckets: HashMap<IpAddr, TokenBucket>,
}

struct TokenBucket {
    tokens: f64,
    last_refill: Instant,
}

impl InboundIpRateLimiter {
    pub(crate) fn new(rate_per_sec: NonZeroU32, burst: NonZeroU32) -> Self {
        Self {
            rate_per_sec: f64::from(rate_per_sec.get()),
            burst: f64::from(burst.get()),
            buckets: HashMap::new(),
        }
    }

    /// Returns `true` if a new inbound connection from `ip` is permitted, consuming
    /// one token. `now` is taken as an argument to keep the type unit-testable.
    pub(crate) fn check(&mut self, ip: IpAddr, now: Instant) -> bool {
        // Enforce size cap. Idle buckets are reclaimed separately by `evict_idle`.
        if self.buckets.len() >= MAX_TRACKED_IPS && !self.buckets.contains_key(&ip) {
            return false;
        }

        let rate = self.rate_per_sec;
        let burst = self.burst;
        let bucket = self.buckets.entry(ip).or_insert(TokenBucket {
            tokens: burst,
            last_refill: now,
        });

        bucket.tokens = Self::refilled_tokens(bucket, now, rate, burst);
        bucket.last_refill = now;

        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Drop buckets that have refilled to capacity (i.e. have seen no recent
    /// activity); a full bucket is indistinguishable from a never-seen IP, so
    /// forgetting it is safe.
    pub(crate) fn evict_idle(&mut self, now: Instant) {
        let rate = self.rate_per_sec;
        let burst = self.burst;
        self.buckets
            .retain(|_ip, bucket| Self::refilled_tokens(bucket, now, rate, burst) < burst);
    }

    /// Tokens `bucket` holds after refilling for the time elapsed since its last
    /// update, clamped to the burst capacity.
    fn refilled_tokens(bucket: &TokenBucket, now: Instant, rate: f64, burst: f64) -> f64 {
        let elapsed = now
            .saturating_duration_since(bucket.last_refill)
            .as_secs_f64();
        (bucket.tokens + elapsed * rate).min(burst)
    }

    #[cfg(test)]
    fn tracked_ips(&self) -> usize {
        self.buckets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;
    use std::time::Duration;

    fn ip(n: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(127, 0, 0, n))
    }

    fn nz(n: u32) -> NonZeroU32 {
        NonZeroU32::new(n).unwrap()
    }

    #[test]
    fn admits_full_burst_then_throttles() {
        let now = Instant::now();
        let mut limiter = InboundIpRateLimiter::new(nz(10), nz(10));
        for _ in 0..10 {
            assert!(limiter.check(ip(1), now));
        }
        // The next attempt at the same instant is over budget.
        assert!(!limiter.check(ip(1), now));
    }

    #[test]
    fn refills_at_the_configured_rate() {
        let now = Instant::now();
        let mut limiter = InboundIpRateLimiter::new(nz(10), nz(10));
        for _ in 0..10 {
            assert!(limiter.check(ip(1), now));
        }
        assert!(!limiter.check(ip(1), now));

        // After half a second, ~5 tokens (0.5s * 10/s) have refilled.
        let later = now + Duration::from_millis(500);
        let admitted = (0..10).filter(|_| limiter.check(ip(1), later)).count();
        assert_eq!(admitted, 5);
    }

    #[test]
    fn buckets_are_per_ip() {
        let now = Instant::now();
        let mut limiter = InboundIpRateLimiter::new(nz(1), nz(1));
        assert!(limiter.check(ip(1), now));
        assert!(!limiter.check(ip(1), now));
        // A different source IP has an independent bucket.
        assert!(limiter.check(ip(2), now));
        assert!(!limiter.check(ip(2), now));
    }

    #[test]
    fn evict_idle_reclaims_fully_refilled_buckets() {
        let now = Instant::now();
        let mut limiter = InboundIpRateLimiter::new(nz(10), nz(10));
        for n in 0u8..50 {
            assert!(limiter.check(ip(n), now));
        }
        assert_eq!(limiter.tracked_ips(), 50);

        // After enough time for the buckets to refill to capacity, a sweep reclaims
        // them all -- they're indistinguishable from never-seen IPs.
        limiter.evict_idle(now + Duration::from_secs(60));
        assert_eq!(limiter.tracked_ips(), 0);
    }

    #[test]
    fn tracked_ips_never_exceeds_the_cap() {
        let now = Instant::now();
        // rate = burst = 1 leaves every bucket drained (non-full) at `now`, so eviction
        // can reclaim nothing and the cap must be held by rejecting new sources.
        let mut limiter = InboundIpRateLimiter::new(nz(1), nz(1));
        for n in 0..MAX_TRACKED_IPS as u32 {
            assert!(limiter.check(IpAddr::V4(Ipv4Addr::from(n)), now));
        }
        assert_eq!(limiter.tracked_ips(), MAX_TRACKED_IPS);

        // A further distinct source is rejected rather than growing the map past the cap.
        let overflow = IpAddr::V4(Ipv4Addr::from(MAX_TRACKED_IPS as u32));
        assert!(!limiter.check(overflow, now));
        assert_eq!(limiter.tracked_ips(), MAX_TRACKED_IPS);
    }

    #[test]
    fn idle_refill_is_capped_at_burst() {
        let now = Instant::now();
        let mut limiter = InboundIpRateLimiter::new(nz(2), nz(3));
        for _ in 0..3 {
            assert!(limiter.check(ip(1), now));
        }
        assert!(!limiter.check(ip(1), now));

        // A long idle period refills only up to the burst capacity, not beyond.
        let later = now + Duration::from_secs(100);
        let admitted = (0..10).filter(|_| limiter.check(ip(1), later)).count();
        assert_eq!(admitted, 3);
    }
}
