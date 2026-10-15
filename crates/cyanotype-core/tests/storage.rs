//! Permanent deletion against a real S3 API. Needs a bucket: set S3_ENDPOINT, S3_BUCKET, S3_ACCESS_KEY and
//! S3_SECRET_KEY (and S3_FORCE_PATH_STYLE=true for MinIO / SeaweedFS). Without S3_ENDPOINT the tests return early.
//! Versioning is exercised when the bucket has it enabled (the production bucket does); on an unversioned bucket
//! the same assertions hold with one version per key.
use aws_sdk_s3::{
    Client,
    config::{Credentials, Region},
};
use cyanotype_core::{config::S3Config, storage::Storage};
use uuid::Uuid;

fn setup() -> Option<(Storage, Client, String)> {
    std::env::var("S3_ENDPOINT").ok().filter(|v| !v.is_empty())?;
    let cfg = S3Config::from_env().expect("S3_* env");
    let client = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(Region::new(cfg.region.clone()))
            .credentials_provider(Credentials::new(&cfg.access_key, &cfg.secret_key, None, None, "static"))
            .force_path_style(cfg.force_path_style)
            .endpoint_url(cfg.endpoint.clone().unwrap())
            .build(),
    );
    Some((Storage::new(&cfg), client, cfg.bucket))
}

async fn all_keys(client: &Client, bucket: &str, prefix: &str) -> Vec<(String, bool)> {
    // (key, is_delete_marker) for every version and marker under the prefix.
    let out = client
        .list_object_versions()
        .bucket(bucket)
        .prefix(prefix)
        .send()
        .await
        .unwrap();
    let mut v: Vec<_> = out
        .versions()
        .iter()
        .map(|x| (x.key().unwrap().to_string(), false))
        .collect();
    v.extend(
        out.delete_markers()
            .iter()
            .map(|x| (x.key().unwrap().to_string(), true)),
    );
    v.sort();
    v
}

#[tokio::test]
async fn delete_all_versions_removes_old_versions_and_delete_markers_but_only_exact_keys() {
    let Some((storage, client, bucket)) = setup() else {
        return;
    };
    let p = format!("test-purge/{}/", Uuid::new_v4());
    let (a, ax, b) = (format!("{p}a"), format!("{p}ax"), format!("{p}b"));
    for body in ["v1", "v2", "v3"] {
        storage.put_bytes(&a, body.into(), "text/plain").await.unwrap();
    }
    storage.put_bytes(&ax, b"x".to_vec(), "text/plain").await.unwrap(); // shares the prefix `a`
    storage.put_bytes(&b, b"b".to_vec(), "text/plain").await.unwrap();
    // A plain DeleteObject on a versioned bucket only adds a marker; every byte stays.
    client.delete_object().bucket(&bucket).key(&a).send().await.unwrap();
    let before = all_keys(&client, &bucket, &p).await;
    let versioned = before.iter().filter(|(k, _)| k == &a).count() > 1;
    if versioned {
        assert!(
            before.contains(&(a.clone(), true)),
            "a delete marker was added: {before:?}"
        );
    }
    assert!(
        storage.head_size(&a).await.unwrap().is_none(),
        "hidden behind the marker / deleted"
    );

    let n = storage.delete_all_versions(std::slice::from_ref(&a)).await.unwrap();
    assert!(n >= 1);
    let after = all_keys(&client, &bucket, &p).await;
    assert_eq!(
        after,
        vec![(ax.clone(), false), (b.clone(), false)],
        "exact key only; no versions or markers of `a` left"
    );

    // Deleting what is already gone is fine (a retried purge).
    assert_eq!(storage.delete_all_versions(std::slice::from_ref(&a)).await.unwrap(), 0);

    // Prefix variant, including a key that only has a marker.
    client.delete_object().bucket(&bucket).key(&b).send().await.unwrap();
    storage.delete_prefix_all_versions(&p).await.unwrap();
    assert!(all_keys(&client, &bucket, &p).await.is_empty());
}

#[tokio::test]
async fn deletes_more_than_one_batch_of_versions() {
    let Some((storage, client, bucket)) = setup() else {
        return;
    };
    let p = format!("test-purge/{}/", Uuid::new_v4());
    let mut set = tokio::task::JoinSet::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
    for i in 0..1105 {
        let (s, key, permit) = (
            storage.clone(),
            format!("{p}{i:05}"),
            sem.clone().acquire_owned().await.unwrap(),
        );
        set.spawn(async move {
            s.put_bytes(&key, vec![0], "application/octet-stream").await.unwrap();
            drop(permit);
        });
    }
    while set.join_next().await.is_some() {}
    assert_eq!(
        storage.list_versions(&p).await.unwrap().len(),
        1105,
        "pagination past 1000"
    );

    assert_eq!(storage.delete_prefix_all_versions(&p).await.unwrap(), 1105);
    assert!(all_keys(&client, &bucket, &p).await.is_empty());
}
