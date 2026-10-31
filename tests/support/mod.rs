//! A software authenticator for tests: the part of a phone or a security key that holds a passkey
//! and signs for it. It builds what a browser would send back for `navigator.credentials.create`
//! (attestation `none`) and `navigator.credentials.get` (ES256), for any origin it is told.
//!
//! Its fields are public so a test can cheat in one place at a time: report another origin, wind
//! the counter back, send someone else's user handle.

#![allow(dead_code)]

use ciborium::value::Value as Cbor;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Base64url with no padding, which is how WebAuthn writes bytes in JSON.
pub fn b64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn unb64(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut bits, mut have) = (0u32, 0);
    for c in text.bytes() {
        let value = ALPHABET.iter().position(|&a| a == c).expect("not base64url") as u32;
        bits = bits << 6 | value;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((bits >> have & 0xff) as u8);
        }
    }
    out
}

#[derive(Clone)]
pub struct Authenticator {
    key: SigningKey,
    pub credential_id: Vec<u8>,
    /// The id the site gave the account when this credential was made. It is sent back on every
    /// sign-in, which is how a button can sign someone in with no name typed.
    pub user_handle: Option<Vec<u8>>,
    /// The signature counter. Each sign-in adds one first.
    pub counter: u32,
    /// What the browser says the page's origin is. A test changes it to be a phishing page.
    pub origin: String,
    /// The relying party the credential was made for. A different one is a different site.
    pub rp_id: String,
    pub user_verified: bool,
}

impl Authenticator {
    /// A new authenticator for the site at `origin`, such as `http://localhost:8787`.
    pub fn new(origin: &str) -> Self {
        let key = loop {
            if let Ok(key) = SigningKey::from_slice(&rand::random::<[u8; 32]>()) {
                break key;
            }
        };
        let host = origin.split("://").nth(1).unwrap().split([':', '/']).next().unwrap();
        Self {
            key,
            credential_id: rand::random::<[u8; 32]>().to_vec(),
            user_handle: None,
            counter: 0,
            origin: origin.to_string(),
            rp_id: host.to_string(),
            user_verified: true,
        }
    }

    fn rp_id_hash(&self) -> Vec<u8> {
        Sha256::digest(self.rp_id.as_bytes()).to_vec()
    }

    fn client_data(&self, kind: &str, challenge: &str) -> Vec<u8> {
        json!({
            "type": kind,
            "challenge": challenge,
            "origin": self.origin,
            "crossOrigin": false,
        })
        .to_string()
        .into_bytes()
    }

    fn flags(&self, attested: bool) -> u8 {
        let mut flags = 0x01; // user present
        if self.user_verified {
            flags |= 0x04;
        }
        if attested {
            flags |= 0x40;
        }
        flags
    }

    /// Answers a start route's `options` for making a credential: what the page sends to
    /// `register/finish` as `credential`.
    pub fn register(&mut self, options: &Value) -> Value {
        let public_key = &options["publicKey"];
        let challenge = public_key["challenge"].as_str().expect("a challenge");
        self.user_handle = Some(unb64(public_key["user"]["id"].as_str().expect("a user id")));

        let point = self.key.verifying_key().to_sec1_point(false);
        let cose_key = Cbor::Map(vec![
            (Cbor::Integer(1.into()), Cbor::Integer(2.into())),
            (Cbor::Integer(3.into()), Cbor::Integer((-7).into())),
            (Cbor::Integer((-1).into()), Cbor::Integer(1.into())),
            (Cbor::Integer((-2).into()), Cbor::Bytes(point.x().unwrap().to_vec())),
            (Cbor::Integer((-3).into()), Cbor::Bytes(point.y().unwrap().to_vec())),
        ]);
        let mut auth_data = self.rp_id_hash();
        auth_data.push(self.flags(true));
        auth_data.extend(self.counter.to_be_bytes());
        auth_data.extend([0u8; 16]); // aaguid
        auth_data.extend((self.credential_id.len() as u16).to_be_bytes());
        auth_data.extend(&self.credential_id);
        ciborium::into_writer(&cose_key, &mut auth_data).unwrap();

        let attestation = Cbor::Map(vec![
            (Cbor::Text("fmt".into()), Cbor::Text("none".into())),
            (Cbor::Text("attStmt".into()), Cbor::Map(vec![])),
            (Cbor::Text("authData".into()), Cbor::Bytes(auth_data)),
        ]);
        let mut attestation_object = Vec::new();
        ciborium::into_writer(&attestation, &mut attestation_object).unwrap();

        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": {
                "attestationObject": b64(&attestation_object),
                "clientDataJSON": b64(&self.client_data("webauthn.create", challenge)),
            },
            "clientExtensionResults": {},
        })
    }

    /// Answers a start route's `options` for signing in: what the page sends to `login/finish`
    /// as `credential`. The counter goes up by one first, as a real authenticator's does.
    pub fn sign_in(&mut self, options: &Value) -> Value {
        let challenge = options["publicKey"]["challenge"].as_str().expect("a challenge");
        self.counter += 1;
        self.assert_challenge(challenge)
    }

    /// An assertion over a challenge, at whatever the counter says now.
    pub fn assert_challenge(&self, challenge: &str) -> Value {
        let client_data = self.client_data("webauthn.get", challenge);
        let mut auth_data = self.rp_id_hash();
        auth_data.push(self.flags(false));
        auth_data.extend(self.counter.to_be_bytes());

        let mut signed = auth_data.clone();
        signed.extend(Sha256::digest(&client_data));
        let signature: Signature = self.key.sign(&signed);

        let mut response = json!({
            "authenticatorData": b64(&auth_data),
            "clientDataJSON": b64(&client_data),
            "signature": b64(signature.to_der().as_bytes()),
        });
        if let Some(handle) = &self.user_handle {
            response["userHandle"] = json!(b64(handle));
        }
        json!({
            "id": b64(&self.credential_id),
            "rawId": b64(&self.credential_id),
            "type": "public-key",
            "response": response,
            "clientExtensionResults": {},
        })
    }
}

/// Changes one byte of a base64url field in a response, so what it says no longer matches what
/// was signed.
pub fn tamper(credential: &mut Value, field: &str) {
    let text = credential["response"][field].as_str().expect("a field").to_string();
    let mut bytes = unb64(&text);
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    credential["response"][field] = json!(b64(&bytes));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_round_trips() {
        for len in 0..40usize {
            let bytes: Vec<u8> = (0..len).map(|n| (n * 37 + 11) as u8).collect();
            assert_eq!(unb64(&b64(&bytes)), bytes, "{len} bytes");
        }
        assert_eq!(b64(&[0xfb, 0xff]), "-_8");
    }
}
