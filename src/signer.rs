use std::path::PathBuf;

use data_encoding::BASE64URL;
use jsonwebkey::JsonWebKey;
use rand::thread_rng;
use rsa::{pkcs8::FromPublicKey, PaddingScheme, PublicKey, RsaPublicKey};
use sha2::Digest;

use crate::{
    crypto::{base64::Base64, hash::ToItems, Provider},
    error::Error,
    transaction::Tx,
};

pub struct ArweaveSigner {
    crypto: Box<Provider>,
}

impl Default for ArweaveSigner {
    fn default() -> Self {
        Self {
            crypto: Box::new(Provider::default()),
        }
    }
}

impl ArweaveSigner {
    pub fn verify(pub_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), Error> {
        let crypto = Provider::default();
        match crypto.verify(pub_key, message, signature) {
            true => Ok(()),
            false => Err(Error::InvalidSignature),
        }
    }

    pub fn from_keypair_path(keypair_path: PathBuf) -> Result<ArweaveSigner, Error> {
        let crypto = Provider::from_keypair_path(keypair_path);
        let signer = ArweaveSigner {
            crypto: Box::new(crypto),
        };
        Ok(signer)
    }

    pub fn sign_transaction(&self, mut transaction: Tx) -> Result<Tx, Error> {
        let deep_hash_item = transaction
            .to_deep_hash_item()
            .expect("Could not convert transaction into deep hash item");
        let signature_data = self.crypto.deep_hash(deep_hash_item);
        let signature = self.crypto.sign(&signature_data);
        let id = self.crypto.hash_sha256(&signature.0);
        transaction.signature = signature;
        transaction.id = Base64(id.to_vec());
        Ok(transaction)
    }

    pub fn sign(&self, message: &[u8]) -> Base64 {
        self.crypto.sign(message)
    }

    pub fn verify_transaction(transaction: &Tx) -> Result<(), Error> {
        if transaction.signature.is_empty() {
            return Err(Error::UnsignedTransaction);
        }

        let crypto = Provider::default();
        let deep_hash_item = transaction
            .to_deep_hash_item()
            .expect("Could not convert transaction into deep hash item");
        let message = crypto.deep_hash(deep_hash_item);
        let signature = &transaction.signature;

        let jwt_str = format!(
            "{{\"kty\":\"RSA\",\"e\":\"AQAB\",\"n\":\"{}\"}}",
            BASE64URL.encode(&transaction.owner.0)
        );
        let jwk: JsonWebKey = jwt_str.parse().unwrap();

        let pub_key = RsaPublicKey::from_public_key_der(jwk.key.to_der().as_slice()).unwrap();
        let mut hasher = sha2::Sha256::new();
        hasher.update(&message);
        let hashed = &hasher.finalize();

        let rng = thread_rng();
        let padding = PaddingScheme::PSS {
            salt_rng: Box::new(rng),
            digest: Box::new(sha2::Sha256::new()),
            salt_len: None,
        };
        pub_key
            .verify(padding, hashed.as_ref(), &signature.0)
            .map(|_| ())
            .map_err(|_| Error::InvalidSignature)
    }

    pub fn wallet_address(&self) -> Base64 {
        self.crypto.wallet_address()
    }

    pub fn keypair_modulus(&self) -> Base64 {
        self.crypto.keypair_modulus()
    }

    pub fn get_provider(&self) -> &Provider {
        &self.crypto
    }

    pub fn get_public_key(&self) -> Base64 {
        self.crypto.public_key()
    }
}

#[cfg(test)]
mod tests {
    use crate::error::Error;

    use super::{ArweaveSigner, Base64};

    #[test]
    fn test_sign_verify() -> Result<(), Error> {
        let message = Base64(
            [
                74, 15, 74, 255, 248, 205, 47, 229, 107, 195, 69, 76, 215, 249, 34, 186, 197, 31,
                178, 163, 72, 54, 78, 179, 19, 178, 1, 132, 183, 231, 131, 213, 146, 203, 6, 99,
                106, 231, 215, 199, 181, 171, 52, 255, 205, 55, 203, 117,
            ]
            .to_vec(),
        );
        let signer = ArweaveSigner::default();
        let signature = signer.sign(&message.0);
        let pubk = signer.get_public_key();
        ArweaveSigner::verify(&pubk.0, &message.0, &signature.0)
    }
}
