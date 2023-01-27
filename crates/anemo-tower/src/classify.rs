use anemo::{
    types::{header, response::StatusCode},
    Response,
};
use std::{fmt, ops::RangeInclusive};

/// Trait for classifying responses as either success or failure.
///
/// Response classifiers are used in cases where middleware needs to determine
/// whether a response completed successfully or failed. For example, they may
/// be used by logging or metrics middleware to record failures differently
/// from successes.
///
/// Furthermore, when a response fails, a response classifier may provide
/// additional information about the failure.
pub trait Classifier {
    /// The type returned when a response is classified as a failure.
    ///
    /// Depending on the classifier, this may simply indicate that the
    /// request failed, or it may contain additional  information about
    /// the failure, such as whether or not it is retryable.
    type FailureClass;

    /// Classify a response.
    fn classify_response<B>(self, res: &Response<B>) -> Result<(), Self::FailureClass>;

    /// Classify an error.
    ///
    /// Errors are always errors (doh) but sometimes it might be useful to have multiple classes of
    /// errors. A retry policy might allow retrying some errors and not others.
    fn classify_error<E>(self, error: &E) -> Self::FailureClass
    where
        E: fmt::Display + 'static;
}

/// Classifier that considers responses with a status code within some range to be failures.
#[derive(Debug, Clone)]
pub struct StatusInRangeAsFailures {
    range: RangeInclusive<u16>,
}

impl StatusInRangeAsFailures {
    /// Creates a new `StatusInRangeAsFailures` that classifies responses with a `5xx` status code
    /// as failures, all others are considered successes.
    pub fn new_for_server_errors() -> Self {
        Self { range: 500..=599 }
    }

    /// Creates a new `StatusInRangeAsFailures` that classifies responses with a `4xx` or `5xx`
    /// status code as failures, all others are considered successes.
    pub fn new_for_client_and_server_errors() -> Self {
        Self { range: 400..=599 }
    }
}

impl Classifier for StatusInRangeAsFailures {
    type FailureClass = StatusInRangeFailureClass;

    fn classify_response<B>(self, response: &Response<B>) -> Result<(), Self::FailureClass> {
        if self.range.contains(&response.status().to_u16()) {
            let class = StatusInRangeFailureClass::StatusCode {
                code: response.status(),
                status_message: response
                    .headers()
                    .get(header::STATUS_MESSAGE)
                    .map(ToOwned::to_owned),
            };
            Err(class)
        } else {
            Ok(())
        }
    }

    fn classify_error<E>(self, error: &E) -> Self::FailureClass
    where
        E: fmt::Display + 'static,
    {
        StatusInRangeFailureClass::Error(error.to_string())
    }
}

/// The failure class for [`StatusInRangeAsFailures`].
#[derive(Debug)]
pub enum StatusInRangeFailureClass {
    /// A response was classified as a failure with the corresponding status.
    StatusCode {
        code: StatusCode,
        status_message: Option<String>,
    },
    /// A response was classified as an error with the corresponding error description.
    Error(String),
}

impl fmt::Display for StatusInRangeFailureClass {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::StatusCode {
                code,
                status_message,
            } => {
                write!(f, "Status code: {code}")?;
                if let Some(status_message) = status_message {
                    write!(f, " {status_message}")?;
                }
                Ok(())
            }
            Self::Error(error) => write!(f, "Error: {error}"),
        }
    }
}
