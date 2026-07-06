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
    #[error("object storage error: {0}")]
    S3(String),
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
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type(content_type)
            .body(ByteStream::from(bytes))
            .send()
            .await
            .map_err(|error| ObjectStorageError::S3(error.to_string()))?;

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
            .map_err(|error| ObjectStorageError::S3(error.to_string()))?;
        let bytes = object
            .body
            .collect()
            .await
            .map_err(|error| ObjectStorageError::S3(error.to_string()))?
            .into_bytes();

        Ok(bytes.to_vec())
    }

    pub fn public_url(&self, key: &str) -> Option<String> {
        self.public_base_url
            .as_ref()
            .map(|base_url| format!("{}/{}", base_url.trim_end_matches('/'), key))
    }
}
