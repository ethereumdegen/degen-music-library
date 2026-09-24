use crate::storage::ConnectionDetails;
use anyhow::{Context, Result};
use keyring::v1::{Entry, Error};
use zeroize::Zeroize;

const SERVICE: &str = "degen-music-library";
const ACCOUNT: &str = "default-s3-connection";

pub fn load() -> Result<Option<ConnectionDetails>> {
  let entry = entry()?;
  let mut payload = match entry.get_secret() {
    Ok(payload) => payload,
    Err(Error::NoEntry) => return Ok(None),
    Err(error) => return Err(error).context("reading credentials from the OS keyring"),
  };
  let details = serde_json::from_slice(&payload).context("decoding saved credentials");
  payload.zeroize();
  details.map(Some)
}

pub fn save(details: &ConnectionDetails) -> Result<()> {
  let entry = entry()?;
  let mut payload = serde_json::to_vec(details).context("encoding credentials")?;
  let result = entry
    .set_secret(&payload)
    .context("saving credentials in the OS keyring");
  payload.zeroize();
  result
}

pub fn forget() -> Result<()> {
  match entry()?.delete_credential() {
    Ok(()) | Err(Error::NoEntry) => Ok(()),
    Err(error) => Err(error).context("deleting credentials from the OS keyring"),
  }
}

fn entry() -> Result<Entry> {
  Entry::new(SERVICE, ACCOUNT).context("opening the OS keyring")
}
