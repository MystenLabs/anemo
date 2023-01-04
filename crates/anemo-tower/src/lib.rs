pub mod auth;
pub mod callback;
pub mod classify;
pub mod inflight_limit;
pub mod rate_limit;
pub mod request_id;
pub mod set_header;
pub mod trace;

/// The latency unit used to report latencies by middleware.
#[non_exhaustive]
#[derive(Copy, Clone, Debug)]
pub enum LatencyUnit {
    /// Use seconds.
    Seconds,
    /// Use milliseconds.
    Millis,
    /// Use microseconds.
    Micros,
    /// Use nanoseconds.
    Nanos,
}

impl LatencyUnit {
    pub fn display(self, latency: std::time::Duration) -> impl std::fmt::Display {
        LatencyDisplay {
            unit: self,
            latency,
        }
    }
}

struct LatencyDisplay {
    unit: LatencyUnit,
    latency: std::time::Duration,
}

impl std::fmt::Display for LatencyDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.unit {
            LatencyUnit::Seconds => write!(f, "{} s", self.latency.as_secs_f64()),
            LatencyUnit::Millis => write!(f, "{} ms", self.latency.as_millis()),
            LatencyUnit::Micros => write!(f, "{} μs", self.latency.as_micros()),
            LatencyUnit::Nanos => write!(f, "{} ns", self.latency.as_nanos()),
        }
    }
}
