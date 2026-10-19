//! The S3 client's HTTPS stack (rustls with aws-lc-rs, platform root certificates). Ignored by default because it
//! needs outbound network access: `cargo test -p cyanotype-core --test tls -- --ignored`.
//! It sends an unauthenticated-looking request with throwaway credentials to a public endpoint; any answer from
//! the service (403/404 included) proves the TLS handshake and certificate validation worked, while a failure to
//! connect (`DispatchFailure`) means the client has no crypto provider or no trusted roots.
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
    error::SdkError,
};

#[tokio::test]
#[ignore = "needs outbound network access"]
async fn https_handshake_with_public_endpoint() {
    let client = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new("x", "x", None, None, "static"))
            .build(),
    );
    let err = client
        .head_bucket()
        .bucket("cyanotype-tls-check-does-not-exist")
        .send()
        .await
        .expect_err("a made-up bucket with throwaway credentials cannot succeed");
    match err {
        SdkError::ServiceError(e) => {
            let status = e.raw().status().as_u16();
            assert!((400..500).contains(&status), "unexpected status {status}");
        }
        other => panic!("no TLS answer from the service: {other:?}"),
    }
}
