/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use aws_smithy_http::result::SdkError;
use aws_smithy_runtime_api::client::interceptors::InterceptorContext;
use aws_smithy_runtime_api::client::retries::{ClassifyRetry, RetryReason};
use aws_smithy_types::retry::{ErrorKind, ProvideErrorKind};
use std::borrow::Cow;
use std::error::Error as StdError;
use std::marker::PhantomData;

/// A retry classifier for checking if an error is modeled as retryable.
#[derive(Debug)]
pub struct ModeledAsRetryableClassifier<E>
where
    E: StdError + ProvideErrorKind + Send + Sync + 'static,
{
    _inner: PhantomData<E>,
}

impl<E> ModeledAsRetryableClassifier<E>
where
    E: StdError + ProvideErrorKind + Send + Sync + 'static,
{
    /// Create a new ModeledAsRetryableClassifier
    pub fn new() -> Self {
        Self {
            _inner: PhantomData,
        }
    }
}

impl<E> ClassifyRetry for ModeledAsRetryableClassifier<E>
where
    E: StdError + ProvideErrorKind + Send + Sync + 'static,
{
    fn classify_retry(&self, ctx: &InterceptorContext) -> Option<RetryReason> {
        // Check for a result
        let output_or_error = ctx.output_or_error()?;
        // Check for an error
        let error = match output_or_error {
            Ok(_) => return None,
            Err(err) => err,
        };
        // Check that the error is an operation error
        let error = error.as_operation_error()?;
        // Downcast the error
        let error = error.downcast_ref::<E>()?;
        // Check if the error is retryable
        error.retryable_error_kind().map(RetryReason::Error)
    }

    fn name(&self) -> &'static str {
        "Errors Modeled As Retryable"
    }
}

#[derive(Debug)]
pub struct SmithyErrorClassifier<E>
where
    E: StdError + Send + Sync + 'static,
{
    _inner: PhantomData<E>,
}

impl<E> SmithyErrorClassifier<E>
where
    E: StdError + Send + Sync + 'static,
{
    /// Create a new SmithyErrorClassifier
    pub fn new() -> Self {
        Self {
            _inner: PhantomData,
        }
    }
}

impl<E> ClassifyRetry for SmithyErrorClassifier<E>
where
    E: StdError + Send + Sync + 'static,
{
    fn classify_retry(&self, ctx: &InterceptorContext) -> Option<RetryReason> {
        let output_or_error = ctx.output_or_error()?;
        // Check for an error
        let error = match output_or_error {
            Ok(_) => return None,
            Err(err) => err,
        };
        // Check that the error is an operation error
        let error = error.as_operation_error()?;
        // Downcast the error
        let error = error.downcast_ref::<SdkError<E>>()?;
        match error {
            SdkError::TimeoutError(_) => Some(RetryReason::Error(ErrorKind::TransientError)),
            SdkError::ResponseError { .. } => Some(RetryReason::Error(ErrorKind::TransientError)),
            SdkError::DispatchFailure(err) if (err.is_timeout() || err.is_io()) => {
                Some(RetryReason::Error(ErrorKind::TransientError))
            }
            SdkError::DispatchFailure(err) => err.is_other().map(RetryReason::Error),
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        "Retryable Smithy Errors"
    }
}

const TRANSIENT_ERROR_STATUS_CODES: &[u16] = &[500, 502, 503, 504];

/// A retry classifier that will treat HTTP response with those status codes as retryable.
/// The `Default` version will retry 500, 502, 503, and 504 errors.
#[derive(Debug)]
pub struct HttpStatusCodeClassifier {
    retryable_status_codes: Cow<'static, [u16]>,
}

impl HttpStatusCodeClassifier {
    /// Given a `Vec<u16>` where the `u16`s represent status codes, create a retry classifier that will
    /// treat HTTP response with those status codes as retryable. The `Default` version will retry
    /// 500, 502, 503, and 504 errors.
    pub fn new_from_codes(retryable_status_codes: impl Into<Cow<'static, [u16]>>) -> Self {
        Self {
            retryable_status_codes: retryable_status_codes.into(),
        }
    }
}

impl ClassifyRetry for HttpStatusCodeClassifier {
    fn classify_retry(&self, ctx: &InterceptorContext) -> Option<RetryReason> {
        ctx.response()
            .map(|res| res.status().as_u16())
            .map(|status| self.retryable_status_codes.contains(&status))
            .unwrap_or_default()
            .then_some(RetryReason::Error(ErrorKind::TransientError))
    }

    fn name(&self) -> &'static str {
        "HTTP Status Code"
    }
}

impl Default for HttpStatusCodeClassifier {
    fn default() -> Self {
        Self::new_from_codes(TRANSIENT_ERROR_STATUS_CODES.to_owned())
    }
}

// Generic smithy clients would have something like this:
// pub fn default_retry_classifiers() -> RetryClassifiers {
//     RetryClassifiers::new()
//         .with_classifier(SmithyErrorClassifier::new())
//         .with_classifier(ModeledAsRetryableClassifier::new())
//         .with_classifier(HttpStatusCodeClassifier::new())
// }
// This ordering is different than the default AWS ordering because the old generic client classifier
// was the same.

#[cfg(test)]
mod test {
    use crate::client::retries::classifier::{
        HttpStatusCodeClassifier, ModeledAsRetryableClassifier,
    };
    use aws_smithy_http::body::SdkBody;
    use aws_smithy_http::operation;
    use aws_smithy_http::result::SdkError;
    use aws_smithy_runtime_api::client::interceptors::InterceptorContext;
    use aws_smithy_runtime_api::client::orchestrator::OrchestratorError;
    use aws_smithy_runtime_api::client::retries::{ClassifyRetry, RetryReason};
    use aws_smithy_runtime_api::type_erasure::{TypeErasedBox, TypeErasedError};
    use aws_smithy_types::retry::{ErrorKind, ProvideErrorKind};
    use std::fmt;

    use super::SmithyErrorClassifier;

    #[derive(Debug, PartialEq, Eq, Clone)]
    struct UnmodeledError;

    impl fmt::Display for UnmodeledError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "UnmodeledError")
        }
    }

    impl std::error::Error for UnmodeledError {}

    #[test]
    fn classify_by_response_status() {
        let policy = HttpStatusCodeClassifier::default();
        let res = http::Response::builder()
            .status(500)
            .body("error!")
            .unwrap()
            .map(SdkBody::from);
        let mut ctx = InterceptorContext::new(TypeErasedBox::doesnt_matter());
        ctx.set_response(res);
        assert_eq!(
            policy.classify_retry(&ctx),
            Some(RetryReason::Error(ErrorKind::TransientError))
        );
    }

    #[test]
    fn classify_by_response_status_not_retryable() {
        let policy = HttpStatusCodeClassifier::default();
        let res = http::Response::builder()
            .status(408)
            .body("error!")
            .unwrap()
            .map(SdkBody::from);
        let mut ctx = InterceptorContext::new(TypeErasedBox::doesnt_matter());
        ctx.set_response(res);
        assert_eq!(policy.classify_retry(&ctx), None);
    }

    #[test]
    fn classify_by_error_kind() {
        #[derive(Debug)]
        struct RetryableError;

        impl fmt::Display for RetryableError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "Some retryable error")
            }
        }

        impl ProvideErrorKind for RetryableError {
            fn retryable_error_kind(&self) -> Option<ErrorKind> {
                Some(ErrorKind::ClientError)
            }

            fn code(&self) -> Option<&str> {
                // code should not be called when `error_kind` is provided
                unimplemented!()
            }
        }

        impl std::error::Error for RetryableError {}

        let policy = ModeledAsRetryableClassifier::<RetryableError>::new();
        let mut ctx = InterceptorContext::new(TypeErasedBox::doesnt_matter());
        ctx.set_output_or_error(Err(OrchestratorError::operation(TypeErasedError::new(
            RetryableError,
        ))));

        assert_eq!(
            policy.classify_retry(&ctx),
            Some(RetryReason::Error(ErrorKind::ClientError)),
        );
    }

    #[test]
    fn classify_response_error() {
        let policy = SmithyErrorClassifier::<UnmodeledError>::new();
        let test_response = http::Response::new("OK").map(SdkBody::from);
        let err: SdkError<UnmodeledError> =
            SdkError::response_error(UnmodeledError, operation::Response::new(test_response));
        let mut ctx = InterceptorContext::new(TypeErasedBox::doesnt_matter());
        ctx.set_output_or_error(Err(OrchestratorError::operation(TypeErasedError::new(err))));
        assert_eq!(
            policy.classify_retry(&ctx),
            Some(RetryReason::Error(ErrorKind::TransientError)),
        );
    }

    #[test]
    fn test_timeout_error() {
        let policy = SmithyErrorClassifier::<UnmodeledError>::new();
        let err: SdkError<UnmodeledError> = SdkError::timeout_error("blah");
        let mut ctx = InterceptorContext::new(TypeErasedBox::doesnt_matter());
        ctx.set_output_or_error(Err(OrchestratorError::operation(TypeErasedError::new(err))));
        assert_eq!(
            policy.classify_retry(&ctx),
            Some(RetryReason::Error(ErrorKind::TransientError)),
        );
    }
}
