use anyhow::{Context, Result};
use aws_sdk_s3::config::{Credentials, Region};
use aws_sdk_s3::Client;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use url::Url;
use zeroize::Zeroize;

const AUDIO_EXTENSIONS: &[&str] = &[
  "aac", "aif", "aiff", "alac", "flac", "m4a", "mka", "mp2", "mp3", "ogg", "oga", "opus", "wav",
  "wave", "webm",
];

#[derive(Clone, Serialize, Deserialize)]
pub struct ConnectionDetails {
  pub endpoint: String,
  pub region: String,
  pub bucket: String,
  pub access_key: String,
  pub secret_key: String,
}

impl Drop for ConnectionDetails {
  fn drop(&mut self) {
    self.access_key.zeroize();
    self.secret_key.zeroize();
  }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
  Folder,
  Track,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
  pub kind: EntryKind,
  pub key: String,
  pub name: String,
  pub size: i64,
}

pub struct DownloadedTrack {
  pub file: tempfile::NamedTempFile,
  pub byte_len: u64,
  pub content_type: Option<String>,
  pub extension: Option<String>,
}

#[derive(Clone)]
pub struct Storage {
  client: Client,
  bucket: String,
}

impl Storage {
  pub fn connect(details: &ConnectionDetails) -> Result<Self> {
    let endpoint = normalize_endpoint(&details.endpoint)?;
    let region = details.region.trim();
    let bucket = details.bucket.trim();
    let access_key = details.access_key.trim();

    if region.is_empty() {
      anyhow::bail!("Region is required (Cloudflare R2 uses 'auto')");
    }
    if bucket.is_empty() {
      anyhow::bail!("Bucket name is required");
    }
    if access_key.is_empty() || details.secret_key.is_empty() {
      anyhow::bail!("Access key ID and secret access key are required");
    }

    let credentials = Credentials::new(
      access_key,
      details.secret_key.clone(),
      None,
      None,
      "degen-music-library",
    );
    let config = aws_sdk_s3::config::Builder::new()
      .behavior_version_latest()
      .region(Region::new(region.to_owned()))
      .credentials_provider(credentials)
      .endpoint_url(endpoint)
      .force_path_style(true)
      .build();

    Ok(Self {
      client: Client::from_conf(config),
      bucket: bucket.to_owned(),
    })
  }

  pub fn bucket(&self) -> &str {
    &self.bucket
  }

  pub async fn list(&self, prefix: &str) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut continuation = None;

    loop {
      let mut request = self
        .client
        .list_objects_v2()
        .bucket(&self.bucket)
        .prefix(prefix)
        .delimiter("/");
      if let Some(token) = continuation.as_deref() {
        request = request.continuation_token(token);
      }

      let output = request
        .send()
        .await
        .with_context(|| format!("listing s3://{}/{}", self.bucket, prefix))?;

      entries.extend(output.common_prefixes().iter().filter_map(|item| {
        let key = item.prefix()?.to_owned();
        let name = key
          .strip_prefix(prefix)
          .unwrap_or(&key)
          .trim_end_matches('/')
          .to_owned();
        (!name.is_empty()).then_some(Entry {
          kind: EntryKind::Folder,
          key,
          name,
          size: 0,
        })
      }));

      entries.extend(output.contents().iter().filter_map(|object| {
        let key = object.key()?;
        if key == prefix || key.ends_with('/') || !is_audio_key(key) {
          return None;
        }
        let name = key.strip_prefix(prefix).unwrap_or(key).to_owned();
        if name.contains('/') {
          return None;
        }
        Some(Entry {
          kind: EntryKind::Track,
          key: key.to_owned(),
          name,
          size: object.size().unwrap_or_default(),
        })
      }));

      if !output.is_truncated().unwrap_or(false) {
        break;
      }
      continuation = output.next_continuation_token().map(str::to_owned);
      if continuation.is_none() {
        anyhow::bail!("S3 returned a truncated listing without a continuation token");
      }
    }

    entries.sort_by(|left, right| {
      kind_order(left.kind)
        .cmp(&kind_order(right.kind))
        .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    entries.dedup_by(|left, right| left.kind == right.kind && left.key == right.key);
    Ok(entries)
  }

  pub async fn download(&self, key: &str) -> Result<DownloadedTrack> {
    let output = self
      .client
      .get_object()
      .bucket(&self.bucket)
      .key(key)
      .send()
      .await
      .with_context(|| format!("downloading s3://{}/{}", self.bucket, key))?;

    let content_type = output.content_type().map(str::to_owned);
    let extension = key
      .rsplit_once('.')
      .map(|(_, extension)| extension.to_ascii_lowercase());
    let mut body = output.body;
    let temporary = tempfile::NamedTempFile::new().context("creating temporary audio file")?;
    let file = temporary
      .reopen()
      .context("opening temporary audio file for download")?;
    let mut file = tokio::fs::File::from_std(file);
    let mut byte_len = 0_u64;

    while let Some(chunk) = body.next().await {
      let chunk = chunk.context("reading audio object")?;
      byte_len = byte_len
        .checked_add(chunk.len() as u64)
        .context("audio object is too large")?;
      file
        .write_all(&chunk)
        .await
        .context("writing temporary audio file")?;
    }
    file
      .flush()
      .await
      .context("flushing temporary audio file")?;

    drop(file.into_std().await);
    Ok(DownloadedTrack {
      file: temporary,
      byte_len,
      content_type,
      extension,
    })
  }
}

fn normalize_endpoint(endpoint: &str) -> Result<String> {
  let endpoint = endpoint.trim();
  if endpoint.is_empty() {
    anyhow::bail!("S3 endpoint is required");
  }
  let endpoint = if endpoint.contains("://") {
    endpoint.to_owned()
  } else {
    format!("https://{endpoint}")
  };
  let parsed = Url::parse(&endpoint).context("invalid S3 endpoint")?;
  if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
    anyhow::bail!("S3 endpoint must be an HTTP or HTTPS URL");
  }
  Ok(endpoint.trim_end_matches('/').to_owned())
}

fn is_audio_key(key: &str) -> bool {
  key.rsplit_once('.').is_some_and(|(_, extension)| {
    AUDIO_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
  })
}

fn kind_order(kind: EntryKind) -> u8 {
  match kind {
    EntryKind::Folder => 0,
    EntryKind::Track => 1,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn endpoint_defaults_to_https_and_drops_trailing_slash() {
    assert_eq!(
      normalize_endpoint("account.r2.cloudflarestorage.com/").unwrap(),
      "https://account.r2.cloudflarestorage.com"
    );
  }

  #[test]
  fn audio_filter_is_case_insensitive_and_rejects_other_objects() {
    assert!(is_audio_key("Albums/Track.MP3"));
    assert!(is_audio_key("Albums/Track.flac"));
    assert!(!is_audio_key("Albums/cover.jpg"));
    assert!(!is_audio_key("Albums/no-extension"));
  }
}
