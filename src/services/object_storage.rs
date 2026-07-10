use std::time::Instant;

use aws_sdk_s3::{
    Client,
    config::{BehaviorVersion, Builder, Credentials, Region},
    primitives::ByteStream,
};

use crate::AppSettings;

#[derive(Clone)]
pub struct ObjectStorage {
    bucket: String,
    client: Client,
    public_base_url: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ObjectStorageError {
    #[error("object storage {operation} failed for {bucket}/{key}")]
    S3 {
        bucket: String,
        key: String,
        operation: &'static str,
    },
}

impl ObjectStorage {
    pub fn new(settings: &AppSettings) -> Self {
        let credentials = Credentials::new(
            settings.object_storage_access_key_id.clone(),
            settings.object_storage_secret_access_key.clone(),
            None,
            None,
            "settings",
        );
        let config = Builder::new()
            .behavior_version(BehaviorVersion::latest())
            .credentials_provider(credentials)
            .endpoint_url(settings.object_storage_endpoint.clone())
            .force_path_style(settings.object_storage_force_path_style)
            .region(Region::new(settings.object_storage_region.clone()))
            .build();

        Self {
            bucket: settings.object_storage_bucket.clone(),
            client: Client::from_conf(config),
            public_base_url: settings
                .object_storage_public_base_url
                .as_ref()
                .filter(|url| !url.trim().is_empty())
                .cloned(),
        }
    }

    pub async fn put(
        &self,
        key: &str,
        bytes: Vec<u8>,
        content_type: &str,
    ) -> Result<(), ObjectStorageError> {
        let byte_len = bytes.len();
        let started = Instant::now();

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    bucket = %self.bucket,
                    key,
                    operation = "put_object",
                    error = ?error,
                    "object storage request failed"
                );
                ObjectStorageError::S3 {
                    bucket: self.bucket.clone(),
                    key: key.to_string(),
                    operation: "put_object",
                }
            })?;

        tracing::info!(
            bucket = %self.bucket,
            key,
            operation = "put_object",
            content_type,
            byte_len,
            duration_ms = started.elapsed().as_millis(),
            "object storage request completed"
        );

        Ok(())
    }

    pub async fn get_bytes(&self, key: &str) -> Result<Vec<u8>, ObjectStorageError> {
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(
                    bucket = %self.bucket,
                    key,
                    operation = "get_object",
                    error = ?error,
                    "object storage request failed"
                );
                ObjectStorageError::S3 {
                    bucket: self.bucket.clone(),
                    key: key.to_string(),
                    operation: "get_object",
                }
            })?;
        let bytes = object
            .body
            .collect()
            .await
            .map_err(|error| {
                tracing::error!(
                    bucket = %self.bucket,
                    key,
                    operation = "read_object_body",
                    error = ?error,
                    "object storage response body failed"
                );
                ObjectStorageError::S3 {
                    bucket: self.bucket.clone(),
                    key: key.to_string(),
                    operation: "read_object_body",
                }
            })?
            .into_bytes();

        Ok(bytes.to_vec())
    }

    pub async fn delete(&self, key: &str) -> Result<(), ObjectStorageError> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|error| {
                tracing::error!(bucket = %self.bucket, key, operation = "delete_object", error = ?error,
                    "object storage request failed");
                ObjectStorageError::S3 {
                    bucket: self.bucket.clone(), key: key.to_string(), operation: "delete_object",
                }
            })?;
        Ok(())
    }

    pub fn public_url(&self, key: &str) -> Option<String> {
        self.public_base_url
            .as_ref()
            .map(|base_url| format!("{}/{}", base_url.trim_end_matches('/'), key))
    }
}
