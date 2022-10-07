use std::{fs::File, path::PathBuf, str::FromStr, time::Duration};

use crypto::{base64::Base64, deep_hash::ToItems, RingProvider};
use error::Error;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::time::sleep;
use transaction::{tags::Tag, Tx};

pub mod client;
pub mod crypto;
pub mod currency;
pub mod error;
pub mod network;
pub mod transaction;
pub mod wallet;

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

/// Block size used for pricing calculations = 256 KB
pub const BLOCK_SIZE: u64 = 1024 * 256;

/// Maximum data size to send to `tx/` endpoint. Sent to `chunk/` endpoint above this.
pub const MAX_TX_DATA: u64 = 10_000_000;

/// Multiplier applied to the buffer argument from the cli to determine the maximum number
/// of simultaneous request to the `chunk/ endpoint`.
pub const CHUNKS_BUFFER_FACTOR: usize = 20;

/// Number of times to retry posting chunks if not successful.
pub const CHUNKS_RETRIES: u16 = 10;

/// Number of seconds to wait between retying to post a failed chunk.
pub const CHUNKS_RETRY_SLEEP: u64 = 1;

#[derive(Serialize, Deserialize, Debug)]
pub struct OraclePrice {
    pub arweave: OraclePricePair,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct OraclePricePair {
    pub usd: f32,
}

pub struct Arweave {
    name: String,
    units: String,
    pub base_url: url::Url,
    pub crypto: Box<dyn crypto::Provider>,
    tx_generator: Box<dyn transaction::generator::Generator>,
}

impl Default for Arweave {
    fn default() -> Self {
        let arweave_url = url::Url::from_str("https://arweave.net/").unwrap();
        Self {
            name: Default::default(),
            units: Default::default(),
            base_url: arweave_url.clone(),
            crypto: Box::new(RingProvider::default()),
            tx_generator: Box::new(Tx::default()),
        }
    }
}

impl Arweave {
    pub fn from_keypair_path(keypair_path: PathBuf, base_url: url::Url) -> Result<Arweave, Error> {
        let crypto = RingProvider::from_keypair_path(keypair_path);
        let arweave = Arweave {
            base_url,
            crypto: Box::new(crypto),
            ..Default::default()
        };
        Ok(arweave)
    }

    pub async fn create_transaction(
        &self,
        target: Base64,
        other_tags: Vec<Tag<Base64>>,
        data: Vec<u8>,
        quantity: u64,
        fee: u64,
        auto_content_tag: bool,
    ) -> Result<Tx, Error> {
        let last_tx = self.get_last_tx().await;
        self.tx_generator.new_tx(
            &*self.crypto,
            target,
            data,
            quantity,
            fee,
            last_tx,
            other_tags,
            auto_content_tag,
        )
    }

    /// Gets deep hash, signs and sets signature and id.
    pub fn sign_transaction(&self, mut transaction: Tx) -> Result<Tx, Error> {
        let deep_hash_item = transaction.to_deep_hash_item().unwrap();
        let signature_data = self.crypto.deep_hash(deep_hash_item);
        let signature = self.crypto.sign(&signature_data);
        let id = self.crypto.hash_sha256(&signature);
        transaction.signature = Base64(signature);
        transaction.id = Base64(id.to_vec());
        Ok(transaction)
    }

    pub fn verify_transaction(&self, transaction: &Tx) -> Result<(), Error> {
        if transaction.signature.is_empty() {
            return Err(Error::UnsignedTransaction);
        }

        let deep_hash_item = transaction.to_deep_hash_item().unwrap();
        let data_to_sign = self.crypto.deep_hash(deep_hash_item);
        let signature = &transaction.signature.to_string();
        let sig_bytes = signature.as_bytes();
        if self.crypto.verify(sig_bytes, &data_to_sign) {
            Ok(())
        } else {
            Err(Error::InvalidSignature)
        }
    }

    pub async fn post_transaction(&self, signed_transaction: &Tx) -> Result<(Base64, u64), Error> {
        if signed_transaction.id.0.is_empty() {
            return Err(error::Error::UnsignedTransaction.into());
        }

        let mut retries = 0;
        let mut status = reqwest::StatusCode::NOT_FOUND;
        let url = self.base_url.join("tx").unwrap();
        let client = reqwest::Client::new();

        while (retries < CHUNKS_RETRIES) & (status != reqwest::StatusCode::OK) {
            let tx_body = json!(&signed_transaction);

            let res = client
                .post(url.clone())
                .json(&signed_transaction)
                .header(&ACCEPT, "application/json")
                .header(&CONTENT_TYPE, "application/json")
                .send()
                .await
                .expect("Could not post transaction");
            status = res.status();
            if status == reqwest::StatusCode::OK {
                return Ok((signed_transaction.id.clone(), signed_transaction.reward));
            }
            sleep(Duration::from_secs(CHUNKS_RETRY_SLEEP)).await;
            retries += 1;
        }

        Err(Error::StatusCodeNotOk)
    }

    async fn get_last_tx(&self) -> Base64 {
        // Fetch and set last_tx if not provided (primarily for testing).
        let resp = reqwest::get(self.base_url.join("tx_anchor").unwrap())
            .await
            .unwrap();
        let last_tx_str = resp.text().await.unwrap();
        Base64::from_str(&last_tx_str).unwrap()
    }

    /// Returns price of uploading data to the network in winstons and USD per AR and USD per SOL
    /// as a BigUint with two decimals.
    pub async fn get_fee(&self, target: Base64) -> Result<u64, Error> {
        let url = self
            .base_url
            .join(&format!("price/0/{}", target.to_string()))
            .unwrap();
        let winstons_per_bytes = reqwest::get(url)
            .await
            .map_err(|e| Error::ArweaveGetPriceError(e.to_string()))?
            .json::<u64>()
            .await
            .unwrap();

        Ok(winstons_per_bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Read, path::PathBuf, str::FromStr};

    use pretend::Url;

    use crate::{error::Error, transaction::Tx, Arweave};

    #[test]
    pub fn should_parse_and_verify_valid_tx() -> Result<(), Error> {
        let mut file = File::open("res/sample_tx.json").unwrap();
        let mut data = String::new();
        file.read_to_string(&mut data).unwrap();
        let tx = Tx::from_str(&data).unwrap();

        let path = PathBuf::from_str("res/test_wallet.json").unwrap();
        let arweave =
            Arweave::from_keypair_path(path, Url::from_str("https://arweave.net").unwrap())
                .unwrap();

        //TODO: verification
        //arweave.verify_transaction(&tx)
        Ok(())
    }
}
