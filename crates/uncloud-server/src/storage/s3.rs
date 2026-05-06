use async_trait::async_trait;
use aws_sdk_s3::config::retry::RetryConfig as AwsRetryConfig;
use aws_sdk_s3::config::{
    BehaviorVersion, Credentials, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use std::path::PathBuf;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

use super::{BoxedAsyncRead, ScanEntry, StorageBackend};
use crate::error::{AppError, Result};

pub struct S3Storage {
    client: Client,
    bucket: String,
    temp_dir: PathBuf,
}

impl S3Storage {
    pub async fn new(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: Option<&str>,
        retry: super::retry::RetryConfig,
    ) -> Result<Self> {
        let region = Region::new(region.unwrap_or("us-east-1").to_string());
        let creds = Credentials::new(access_key, secret_key, None, None, "uncloud-config");

        // `WhenSupported` (the SDK default since v1.81) attaches an
        // `x-amz-checksum-crc32` trailer to every PUT and switches the body
        // to `aws-chunked` transfer encoding. MinIO supports the protocol but
        // has a known buffering bug that adds ~30 s of latency per request,
        // collapsing throughput from MB/s to KB/s. `WhenRequired` only sends
        // the trailer when the operation explicitly demands it (none of ours
        // do), restoring boto3-equivalent performance against MinIO. AWS S3
        // is unaffected because this just opts back into the pre-1.81 wire
        // format. See https://github.com/minio/minio/issues/19528.
        // The AWS SDK's default retry mode is `Standard` with 3 attempts; we
        // drive max_attempts and the initial backoff from our shared
        // RetryConfig so YAML retry knobs apply uniformly to S3 and SFTP.
        let aws_retry = AwsRetryConfig::standard()
            .with_max_attempts(retry.effective_max_attempts())
            .with_initial_backoff(retry.base_delay())
            .with_max_backoff(retry.max_delay());

        let mut conf = aws_sdk_s3::config::Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .region(region)
            .credentials_provider(creds)
            .retry_config(aws_retry)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);

        if !endpoint.is_empty() {
            conf = conf.endpoint_url(endpoint);
        }

        let client = Client::from_conf(conf.build());

        client
            .head_bucket()
            .bucket(bucket)
            .send()
            .await
            .map_err(|e| {
                AppError::Storage(format!("Cannot access S3 bucket '{bucket}': {e}"))
            })?;

        let temp_dir = std::env::temp_dir().join(format!("uncloud-s3-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_dir)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create temp dir: {e}")))?;

        Ok(Self {
            client,
            bucket: bucket.to_string(),
            temp_dir,
        })
    }

    fn key(path: &str) -> String {
        path.trim_start_matches('/').to_string()
    }

    fn temp_local(&self, name: &str) -> PathBuf {
        self.temp_dir.join(name.trim_start_matches('/'))
    }

    async fn copy_object(&self, from: &str, to: &str) -> Result<()> {
        let from_key = Self::key(from);
        let encoded_key: String = from_key
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let copy_source = format!("{}/{}", self.bucket, encoded_key);
        self.client
            .copy_object()
            .bucket(&self.bucket)
            .copy_source(copy_source)
            .key(Self::key(to))
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 copy failed: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl StorageBackend for S3Storage {
    async fn read(&self, path: &str) -> Result<BoxedAsyncRead> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 get failed: {e}")))?;
        Ok(Box::pin(resp.body.into_async_read()))
    }

    async fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<BoxedAsyncRead> {
        if length == 0 {
            return Ok(Box::pin(tokio::io::empty()));
        }
        let end = offset + length - 1;
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .range(format!("bytes={offset}-{end}"))
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 get range failed: {e}")))?;
        Ok(Box::pin(resp.body.into_async_read()))
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        let body = ByteStream::from(data.to_vec());
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 put failed: {e}")))?;
        Ok(())
    }

    async fn write_stream(
        &self,
        path: &str,
        mut reader: BoxedAsyncRead,
        _size: u64,
    ) -> Result<()> {
        let staging = self.temp_dir.join(format!("ws-{}", Uuid::new_v4()));
        {
            let mut file = File::create(&staging)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to create staging file: {e}")))?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = reader
                    .read(&mut buf)
                    .await
                    .map_err(|e| AppError::Storage(format!("Failed to read stream: {e}")))?;
                if n == 0 {
                    break;
                }
                file.write_all(&buf[..n])
                    .await
                    .map_err(|e| AppError::Storage(format!("Failed to write staging: {e}")))?;
            }
            file.sync_all()
                .await
                .map_err(|e| AppError::Storage(format!("Failed to sync staging: {e}")))?;
        }

        let body = ByteStream::from_path(&staging)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open staging body: {e}")))?;
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 put failed: {e}")));
        let _ = fs::remove_file(&staging).await;
        result?;
        Ok(())
    }

    async fn delete(&self, path: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 delete failed: {e}")))?;
        Ok(())
    }

    async fn exists(&self, path: &str) -> Result<bool> {
        let res = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(Self::key(path))
            .send()
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(svc)) => match svc.err() {
                HeadObjectError::NotFound(_) => Ok(false),
                other => Err(AppError::Storage(format!("S3 head failed: {other}"))),
            },
            Err(e) => Err(AppError::Storage(format!("S3 head failed: {e}"))),
        }
    }

    async fn available_space(&self) -> Result<Option<u64>> {
        Ok(None)
    }

    async fn create_temp(&self) -> Result<String> {
        let name = format!("{}.tmp", Uuid::new_v4());
        let path = self.temp_dir.join(&name);
        File::create(&path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to create temp file: {e}")))?;
        Ok(name)
    }

    async fn append_temp(&self, temp_path: &str, data: &[u8]) -> Result<()> {
        let path = self.temp_local(temp_path);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open temp file: {e}")))?;
        file.write_all(data)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to append to temp file: {e}")))?;
        Ok(())
    }

    async fn finalize_temp(&self, temp_path: &str, final_path: &str) -> Result<()> {
        let local = self.temp_local(temp_path);
        let body = ByteStream::from_path(&local)
            .await
            .map_err(|e| AppError::Storage(format!("Failed to open temp body: {e}")))?;
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(final_path))
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Storage(format!("S3 put on finalize failed: {e}")));
        let _ = fs::remove_file(&local).await;
        result?;
        Ok(())
    }

    async fn abort_temp(&self, temp_path: &str) -> Result<()> {
        let path = self.temp_local(temp_path);
        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| AppError::Storage(format!("Failed to abort temp file: {e}")))?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<()> {
        self.copy_object(from, to).await?;
        self.delete(from).await
    }

    async fn archive_version(&self, current: &str, version: &str) -> Result<()> {
        self.copy_object(current, version).await
    }

    async fn move_to_trash(&self, current: &str, trash: &str) -> Result<()> {
        self.rename(current, trash).await
    }

    async fn restore_from_trash(&self, trash: &str, restore: &str) -> Result<()> {
        self.rename(trash, restore).await
    }

    async fn scan(&self, prefix: &str) -> Result<Vec<ScanEntry>> {
        let prefix_key = Self::key(prefix);
        let mut out = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(&prefix_key);
            if let Some(c) = continuation.take() {
                req = req.continuation_token(c);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| AppError::Storage(format!("S3 list failed: {e}")))?;
            for obj in resp.contents() {
                let Some(key) = obj.key() else { continue };
                let size = obj.size().unwrap_or(0).max(0) as u64;
                out.push(ScanEntry {
                    path: key.to_string(),
                    is_dir: false,
                    size_bytes: size,
                });
            }
            if resp.is_truncated().unwrap_or(false) {
                continuation = resp.next_continuation_token().map(|s| s.to_string());
                if continuation.is_none() {
                    break;
                }
            } else {
                break;
            }
        }
        Ok(out)
    }
}
