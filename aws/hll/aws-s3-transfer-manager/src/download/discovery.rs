/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use std::{mem, ops::RangeInclusive, str::FromStr};

use aws_sdk_s3::operation::get_object::builders::GetObjectInputBuilder;
use aws_smithy_types::{
    body::SdkBody,
    byte_stream::{AggregatedBytes, ByteStream},
};
use bytes::Buf;

use crate::error;

use super::{
    handle::DownloadHandle,
    header::{self, ByteRange},
    object_meta::ObjectMetadata,
    DownloadRequest,
};

#[derive(Debug, Clone, PartialEq)]
enum ObjectDiscoveryStrategy {
    // Send a `HeadObject` request.
    // The overall transfer is optionally constrained to the given range.
    HeadObject(Option<ByteRange>),
    // Send `GetObject` with `part_number` = 1
    FirstPart,
    // Send `GetObject` request using a ranged get.
    // The overall transfer is optionally constrained to the given range.
    RangedGet(Option<RangeInclusive<u64>>),
}

/// Discovered object metadata (optionally with first chunk of data)
#[derive(Debug)]
pub(super) struct ObjectDiscovery {
    /// range of data remaining to be fetched
    pub(super) remaining: RangeInclusive<u64>,

    /// the discovered metadata
    pub(super) meta: ObjectMetadata,

    /// the first chunk of data if fetched during discovery
    pub(super) initial_chunk: Option<AggregatedBytes>,
}

impl ObjectDiscoveryStrategy {
    fn from_request(
        request: &DownloadRequest,
    ) -> Result<ObjectDiscoveryStrategy, error::TransferError> {
        let strategy = match request.input.get_range() {
            Some(h) => {
                let byte_range = header::Range::from_str(h)?.0;
                match byte_range {
                    ByteRange::Inclusive(start, end) => {
                        ObjectDiscoveryStrategy::RangedGet(Some(start..=end))
                    }
                    // TODO: explore when given a start range what it would like to just start
                    // sending requests from [start, start+part_size]
                    _ => ObjectDiscoveryStrategy::HeadObject(Some(byte_range)),
                }
            }
            None => ObjectDiscoveryStrategy::RangedGet(None),
        };

        Ok(strategy)
    }
}

pub(super) async fn discover_obj(
    handle: &DownloadHandle,
    request: &DownloadRequest,
) -> Result<ObjectDiscovery, error::TransferError> {
    let strategy = ObjectDiscoveryStrategy::from_request(request)?;
    match strategy {
        ObjectDiscoveryStrategy::HeadObject(byte_range) => {
            discover_obj_with_head(handle, request, byte_range).await
        }
        ObjectDiscoveryStrategy::FirstPart => {
            let r = request.input.clone().part_number(1);
            discover_obj_with_get(handle, r, None).await
        }
        ObjectDiscoveryStrategy::RangedGet(range) => {
            let byte_range = match range.as_ref() {
                Some(r) => ByteRange::Inclusive(*r.start(), *r.start() + handle.target_part_size - 1),
                None => ByteRange::Inclusive(0, handle.target_part_size - 1),
            };
            let r = request
                .input
                .clone()
                .set_part_number(None)
                .range(header::Range::bytes(byte_range));

            discover_obj_with_get(handle, r, range).await
        }
    }
}

async fn discover_obj_with_head(
    handle: &DownloadHandle,
    request: &DownloadRequest,
    byte_range: Option<ByteRange>,
) -> Result<ObjectDiscovery, error::TransferError> {
    let meta: ObjectMetadata = handle
        .client
        .head_object()
        .set_bucket(request.input.get_bucket().clone())
        .set_key(request.input.get_key().clone())
        .send()
        .await
        .map_err(|e| error::DownloadError::DiscoverFailed(e.into()))?
        .into();

    let remaining = match byte_range {
        Some(range) => match range {
            ByteRange::Inclusive(start, end) => start..=end,
            ByteRange::AllFrom(start) => start..=meta.total_size(),
            ByteRange::Last(n) => (meta.total_size() - n + 1)..=meta.total_size(),
        },
        None => 0..=meta.total_size(),
    };

    Ok(ObjectDiscovery {
        remaining,
        meta,
        initial_chunk: None,
    })
}

async fn discover_obj_with_get(
    handle: &DownloadHandle,
    request: GetObjectInputBuilder,
    range: Option<RangeInclusive<u64>>,
) -> Result<ObjectDiscovery, error::TransferError> {
    let resp = request.send_with(&handle.client).await;

    if resp.is_err() {
        // TODO - deal with empty file errors, see https://github.com/awslabs/aws-c-s3/blob/v0.5.7/source/s3_auto_ranged_get.c#L147-L153
    }

    let mut resp = resp.map_err(|e| error::DownloadError::DiscoverFailed(e.into()))?;

    // take the body so we can convert the metadata
    let empty_stream = ByteStream::new(SdkBody::empty());
    let body = mem::replace(&mut resp.body, empty_stream);

    let data = body
        .collect()
        .await
        .map_err(|e| error::DownloadError::DiscoverFailed(e.into()))?;

    let meta: ObjectMetadata = resp.into();

    // TODO - check content size matches range
    let remaining = match range {
        Some(range) => (*range.start() + data.remaining() as u64 + 1)..=*range.end(),
        None => (data.remaining() as u64)..=meta.total_size(),
    };

    Ok(ObjectDiscovery {
        remaining,
        meta,
        initial_chunk: Some(data),
    })
}

#[cfg(test)]
mod tests {
    use std::ops::RangeInclusive;

    use crate::{
        download::{
            discovery::{
                discover_obj, discover_obj_with_get, discover_obj_with_head,
                ObjectDiscoveryStrategy,
            },
            handle::DownloadHandle,
            header::ByteRange,
        },
        MIN_PART_SIZE,
    };
    use aws_sdk_s3::{
        operation::{
            get_object::{GetObjectInput, GetObjectOutput},
            head_object::HeadObjectOutput,
        },
        Client,
    };
    use aws_smithy_mocks_experimental::{mock, mock_client};
    use aws_smithy_types::byte_stream::ByteStream;

    use super::ObjectDiscovery;

    fn strategy_from_range(range: Option<&str>) -> ObjectDiscoveryStrategy {
        let req = GetObjectInput::builder()
            .set_range(range.map(|r| r.to_string()))
            .into();
        ObjectDiscoveryStrategy::from_request(&req).unwrap()
    }

    #[test]
    fn test_stategy_from_req() {
        assert_eq!(
            ObjectDiscoveryStrategy::RangedGet(None),
            strategy_from_range(None)
        );

        assert_eq!(
            ObjectDiscoveryStrategy::RangedGet(Some(100..=200)),
            strategy_from_range(Some("bytes=100-200"))
        );
        assert_eq!(
            ObjectDiscoveryStrategy::HeadObject(Some(ByteRange::AllFrom(100))),
            strategy_from_range(Some("bytes=100-"))
        );
        assert_eq!(
            ObjectDiscoveryStrategy::HeadObject(Some(ByteRange::Last(500))),
            strategy_from_range(Some("bytes=-500"))
        );
    }

    async fn get_discovery_from_head(range: Option<ByteRange>) -> ObjectDiscovery {
        let head_obj_rule = mock!(Client::head_object)
            .then_output(|| HeadObjectOutput::builder().content_length(500).build());
        let client = mock_client!(aws_sdk_s3, &[&head_obj_rule]);

        let handle = DownloadHandle {
            client,
            target_part_size: MIN_PART_SIZE,
        };
        let request = GetObjectInput::builder()
            .bucket("test-bucket")
            .key("test-key")
            .into();

        discover_obj_with_head(&handle, &request, range)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_discover_obj_with_head() {
        assert_eq!(0..=500, get_discovery_from_head(None).await.remaining);
        assert_eq!(
            10..=100,
            get_discovery_from_head(Some(ByteRange::Inclusive(10, 100)))
                .await
                .remaining
        );
        assert_eq!(
            100..=500,
            get_discovery_from_head(Some(ByteRange::AllFrom(100)))
                .await
                .remaining
        );
        assert_eq!(
            401..=500,
            get_discovery_from_head(Some(ByteRange::Last(100)))
                .await
                .remaining
        );
    }

    #[tokio::test]
    async fn test_discover_obj_with_get_full_range() {
        let target_part_size = 500;
        let bytes = &[0u8; 500];
        let get_obj_rule = mock!(Client::get_object)
            .match_requests(|r| r.range() == Some("bytes=0-499"))
            .then_output(|| {
                GetObjectOutput::builder()
                    .content_length(700)
                    .content_range("0-499/700")
                    .body(ByteStream::from_static(bytes))
                    .build()
            });
        let client = mock_client!(aws_sdk_s3, &[&get_obj_rule]);

        let handle = DownloadHandle {
            client,
            target_part_size,
        };

        let request = GetObjectInput::builder()
            .bucket("test-bucket")
            .key("test-key")
            .into();

        let discovery = discover_obj(&handle, &request).await.unwrap();
        assert_eq!(
            500..=700,
            discovery.remaining
        );
    }

    // FIXME - leftoff needing to test sub range (should cover part size with remainging and without)

}
