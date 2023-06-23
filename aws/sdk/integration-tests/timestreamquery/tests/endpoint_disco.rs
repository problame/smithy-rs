/*
 * Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
 * SPDX-License-Identifier: Apache-2.0
 */

use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_timestreamquery as query;
use aws_sdk_timestreamquery::config::Credentials;
use aws_smithy_async::rt::sleep::SharedAsyncSleep;
use aws_smithy_async::test_util::controlled_time_and_sleep;
use aws_smithy_async::time::{SharedTimeSource, TimeSource};
use aws_smithy_client::dvr::{MediaType, ReplayingConnection};
use aws_types::region::Region;
use aws_types::SdkConfig;
use std::time::{Duration, UNIX_EPOCH};

#[tokio::test]
async fn do_endpoint_discovery() {
    tracing_subscriber::fmt::init();
    let conn = ReplayingConnection::from_file("tests/traffic.json").unwrap();
    //let conn = aws_smithy_client::dvr::RecordingConnection::new(conn);
    let start = UNIX_EPOCH + Duration::from_secs(1234567890);
    let (ts, sleep, mut gate) = controlled_time_and_sleep(start);
    let config = SdkConfig::builder()
        .http_connector(conn.clone())
        .region(Region::from_static("us-west-2"))
        .sleep_impl(SharedAsyncSleep::new(sleep))
        .credentials_provider(SharedCredentialsProvider::new(Credentials::for_tests()))
        .time_source(SharedTimeSource::new(ts.clone()))
        .build();
    let conf = query::config::Builder::from(&config)
        .idempotency_token_provider("0000-0000-0000")
        .build();
    let (client, reloader) = query::Client::from_conf(conf)
        .enable_endpoint_discovery()
        .await
        .expect("initial setup of endpoint discovery failed");

    tokio::spawn(reloader.reload_task());

    let _resp = client
        .query()
        .query_string("SELECT now() as time_now")
        .send()
        .await
        .unwrap();

    // wait 10 minutes for the endpoint to expire
    while ts.now() < start + Duration::from_secs(60 * 10) {
        assert_eq!(
            gate.expect_sleep().await.duration(),
            Duration::from_secs(60)
        );
    }

    // the recording validates that this request hits another endpoint
    let _resp = client
        .query()
        .query_string("SELECT now() as time_now")
        .send()
        .await
        .unwrap();
    // if you want to update this test:
    // conn.dump_to_file("tests/traffic.json").unwrap();
    conn.validate_body_and_headers(
        Some(&[
            "x-amz-security-token",
            "x-amz-date",
            "content-type",
            "x-amz-target",
        ]),
        MediaType::Json,
    )
    .await
    .unwrap();
}