//! Multipart upload to S3.
//!
//! An object only exists once `CompleteMultipartUpload` succeeds, and that is
//! only sent after mysqldump has exited 0. Until then the uploaded parts are
//! invisible and are discarded by `AbortMultipartUpload`, so a failed dump can
//! never leave a truncated object behind -- which matters a great deal when
//! the bucket is under Object Lock and a bad object cannot be deleted.

use anyhow::{Context, Result};
use aws_config::meta::region::RegionProviderChain;
use aws_config::timeout::TimeoutConfig;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{RequestChecksumCalculation, ResponseChecksumValidation};
use aws_sdk_s3::error::DisplayErrorContext;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, StorageClass};
use md5::{Digest, Md5};

use crate::config::Config;

#[derive(Clone)]
pub struct Bucket {
    client: Client,
    name: String,
    storage_class: Option<StorageClass>,
}

impl Bucket {
    pub async fn new(cfg: &Config) -> Self {
        // s5cmd fell back to us-east-1 when no region was configured; keep
        // that, since S3-compatible providers often ignore the region.
        let region = RegionProviderChain::default_provider().or_else(Region::new("us-east-1"));
        let shared = aws_config::defaults(BehaviorVersion::latest())
            .region(region)
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_attempt_timeout(cfg.upload_timeout)
                    .build(),
            )
            .load()
            .await;

        // Integrity comes from a Content-MD5 on every part, which AWS and every
        // S3-compatible provider verify. The SDK's default CRC checksums are
        // not supported everywhere, so they are only sent where required.
        let mut s3 = aws_sdk_s3::config::Builder::from(&shared)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
        if let Some(endpoint) = &cfg.endpoint_url {
            // Path-style addressing works on every S3-compatible provider;
            // virtual-hosted style needs wildcard DNS that many do not have.
            s3 = s3.endpoint_url(endpoint).force_path_style(true);
        }

        Bucket {
            client: Client::from_conf(s3.build()),
            name: cfg.bucket.clone(),
            storage_class: cfg.storage_class.as_deref().map(StorageClass::from),
        }
    }

    pub async fn start_upload(&self, key: &str) -> Result<Upload> {
        let mut request = self
            .client
            .create_multipart_upload()
            .bucket(&self.name)
            .key(key)
            .content_type("application/gzip");
        if let Some(class) = &self.storage_class {
            request = request.storage_class(class.clone());
        }
        let output = request
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", DisplayErrorContext(e)))
            .with_context(|| format!("could not start the upload to s3://{}/{key}", self.name))?;
        let upload_id = output
            .upload_id
            .context("CreateMultipartUpload returned no upload ID")?;
        Ok(Upload {
            client: self.client.clone(),
            bucket: self.name.clone(),
            key: key.to_owned(),
            upload_id,
        })
    }
}

/// One in-progress multipart upload. Cheap to clone, so each part can be sent
/// from its own task.
#[derive(Clone)]
pub struct Upload {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
}

impl Upload {
    pub async fn upload_part(&self, number: i32, body: Vec<u8>) -> Result<CompletedPart> {
        let md5 = aws_smithy_types::base64::encode(Md5::digest(&body));
        let output = self
            .client
            .upload_part()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .part_number(number)
            .content_md5(md5)
            .body(ByteStream::from(body))
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", DisplayErrorContext(e)))
            .with_context(|| format!("could not upload part {number}"))?;
        Ok(CompletedPart::builder()
            .part_number(number)
            .set_e_tag(output.e_tag)
            .build())
    }

    pub async fn complete(&self, mut parts: Vec<CompletedPart>) -> Result<()> {
        parts.sort_by_key(|p| p.part_number);
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .multipart_upload(
                CompletedMultipartUpload::builder()
                    .set_parts(Some(parts))
                    .build(),
            )
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", DisplayErrorContext(e)))
            .context("could not complete the upload")?;
        Ok(())
    }

    /// Discards every uploaded part. Best effort: a failure is reported, not
    /// returned, because the backup has already failed by the time this runs.
    pub async fn abort(&self) {
        let result = self
            .client
            .abort_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.upload_id)
            .send()
            .await;
        if let Err(e) = result {
            eprintln!(
                "Warning: could not abort multipart upload {} for s3://{}/{}: {}",
                self.upload_id,
                self.bucket,
                self.key,
                DisplayErrorContext(e)
            );
            eprintln!(
                "Warning: its parts are billed as storage until a lifecycle rule with AbortIncompleteMultipartUpload removes them."
            );
        }
    }
}
