use crate::types::header;
use crate::types::HeaderMap;
use std::fmt;
use std::time::Duration;

pub(crate) mod inbound;
pub(crate) mod outbound;

/// Tries to parse the `timeout` header if it is present. If we fail to parse, returns
/// the value we attempted to parse.
///
/// The value of the `timeout` header is expected to in nanoseconds encoded encoded as an u64
pub(crate) fn try_parse_timeout(headers: &HeaderMap) -> Result<Option<Duration>, &str> {
    match headers.get(header::TIMEOUT) {
        Some(val) => {
            let nanoseconds = val.parse::<u64>().map_err(|_| val.as_ref())?;
            let duration = Duration::from_nanos(nanoseconds);

            Ok(Some(duration))
        }
        None => Ok(None),
    }
}

/// Converts a duration to nanoseconds as an u64 suitable to be used in the `timeout` header field.
pub(crate) fn duration_to_timeout(duration: Duration) -> String {
    let nanoseconds: u64 = duration.as_nanos().try_into().unwrap_or(u64::MAX);
    nanoseconds.to_string()
}

/// Error returned if a request didn't complete within the configured timeout.
#[derive(Debug)]
pub(crate) struct TimeoutExpired(());

impl fmt::Display for TimeoutExpired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timeout expired")
    }
}

// std::error::Error only requires a type to impl Debug and Display
impl std::error::Error for TimeoutExpired {}
