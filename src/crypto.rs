//! RustDesk-compatible NaCl crypto (libsodium wire format via `dryoc`).
//!
//! Used for the host-side secure handshake on relay connections:
//!   * a stable Ed25519 *sign* key pair identifies the host (its public key is
//!     registered with the rendezvous server via `RegisterPk`);
//!   * per connection the host makes an ephemeral *box* (X25519) key pair,
//!     signs `IdPk{ id, box_pk }` and sends it as `SignedId`;
//!   * the peer replies `PublicKey{ their_box_pk, sealed_symmetric_key }`;
//!   * the host opens the sealed key and from then on the TCP stream is
//!     `secretbox` (XSalsa20-Poly1305) encrypted with per-message nonces
//!     derived from a sequence counter.

use dryoc::classic::{
    crypto_box::{crypto_box_keypair, crypto_box_open_easy},
    crypto_secretbox::{crypto_secretbox_easy, crypto_secretbox_open_easy},
    crypto_sign::{crypto_sign, crypto_sign_keypair},
};

pub const SIGN_SK_LEN: usize = 64;
const BOX_PK_LEN: usize = 32;
const BOX_SK_LEN: usize = 32;
const SECRETBOX_KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;
const SIGN_BYTES: usize = 64; // signature prefix length (libsodium combined)
const MAC_BYTES: usize = 16; // box / secretbox Poly1305 tag length

/// Generate a stable Ed25519 sign key pair: `(public[32], secret[64])`.
pub fn gen_sign_keypair() -> (Vec<u8>, Vec<u8>) {
    let (pk, sk) = crypto_sign_keypair();
    (pk.to_vec(), sk.to_vec())
}

/// Ed25519-sign `message` with a 64-byte secret key, libsodium combined mode
/// (output = signature(64) ‖ message). Returns `None` on bad key length.
pub fn sign(message: &[u8], sign_sk: &[u8]) -> Option<Vec<u8>> {
    let sk: [u8; SIGN_SK_LEN] = sign_sk.try_into().ok()?;
    let mut signed = vec![0u8; message.len() + SIGN_BYTES];
    crypto_sign(&mut signed, message, &sk).ok()?;
    Some(signed)
}

/// Ephemeral X25519 box key pair for one connection: `(public[32], secret[32])`.
pub fn gen_box_keypair() -> (Vec<u8>, Vec<u8>) {
    let (pk, sk) = crypto_box_keypair();
    (pk.to_vec(), sk.to_vec())
}

/// Open the symmetric key the peer sealed to our ephemeral box public key.
/// `sealed` = peer's `symmetric_value`, `their_pk` = peer's `asymmetric_value`,
/// `our_box_sk` = our ephemeral box secret. Nonce is all-zero (RustDesk uses
/// a zero nonce for this one sealed message).
pub fn open_symmetric_key(sealed: &[u8], their_pk: &[u8], our_box_sk: &[u8]) -> Option<Vec<u8>> {
    let their: [u8; BOX_PK_LEN] = their_pk.try_into().ok()?;
    let sk: [u8; BOX_SK_LEN] = our_box_sk.try_into().ok()?;
    if sealed.len() < MAC_BYTES {
        return None;
    }
    let nonce = [0u8; NONCE_LEN];
    let mut key = vec![0u8; sealed.len() - MAC_BYTES];
    crypto_box_open_easy(&mut key, sealed, &nonce, &their, &sk).ok()?;
    if key.len() == SECRETBOX_KEY_LEN {
        Some(key)
    } else {
        None
    }
}

/// Stream cipher state for an encrypted relay session.
///
/// `secretbox` with a 24-byte nonce whose first 8 bytes are a little-endian
/// sequence counter (incremented *before* each operation), matching RustDesk's
/// `tcp::Encrypt`.
pub struct StreamCipher {
    key: [u8; SECRETBOX_KEY_LEN],
    send_seq: u64,
    recv_seq: u64,
}

impl StreamCipher {
    pub fn new(key: &[u8]) -> Option<Self> {
        let key: [u8; SECRETBOX_KEY_LEN] = key.try_into().ok()?;
        Some(Self {
            key,
            send_seq: 0,
            recv_seq: 0,
        })
    }

    /// Split into independent send / receive halves (each keeps its own
    /// sequence counter), so a writer thread and a reader thread can run
    /// concurrently without sharing a lock. Counters continue from where the
    /// single-threaded handshake left off.
    pub fn into_halves(self) -> (SendCipher, RecvCipher) {
        (
            SendCipher {
                key: self.key,
                seq: self.send_seq,
            },
            RecvCipher {
                key: self.key,
                seq: self.recv_seq,
            },
        )
    }

    fn nonce(seq: u64) -> [u8; NONCE_LEN] {
        let mut n = [0u8; NONCE_LEN];
        n[..8].copy_from_slice(&seq.to_le_bytes());
        n
    }

    /// Encrypt an outgoing plaintext frame.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.send_seq += 1;
        let nonce = Self::nonce(self.send_seq);
        let mut ct = vec![0u8; plaintext.len() + MAC_BYTES];
        // crypto_secretbox_easy only fails on impossible buffer sizes.
        let _ = crypto_secretbox_easy(&mut ct, plaintext, &nonce, &self.key);
        ct
    }

    /// Decrypt an incoming ciphertext frame.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        self.recv_seq += 1;
        if ciphertext.len() < MAC_BYTES {
            return Err("ciphertext too short".to_owned());
        }
        let nonce = Self::nonce(self.recv_seq);
        let mut pt = vec![0u8; ciphertext.len() - MAC_BYTES];
        crypto_secretbox_open_easy(&mut pt, ciphertext, &nonce, &self.key)
            .map_err(|e| format!("secretbox open: {e:?}"))?;
        Ok(pt)
    }
}

/// Encrypt-only half of a split [`StreamCipher`] (owns the send counter).
pub struct SendCipher {
    key: [u8; SECRETBOX_KEY_LEN],
    seq: u64,
}

impl SendCipher {
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        self.seq += 1;
        let nonce = StreamCipher::nonce(self.seq);
        let mut ct = vec![0u8; plaintext.len() + MAC_BYTES];
        let _ = crypto_secretbox_easy(&mut ct, plaintext, &nonce, &self.key);
        ct
    }
}

/// Decrypt-only half of a split [`StreamCipher`] (owns the receive counter).
pub struct RecvCipher {
    key: [u8; SECRETBOX_KEY_LEN],
    seq: u64,
}

impl RecvCipher {
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, String> {
        self.seq += 1;
        if ciphertext.len() < MAC_BYTES {
            return Err("ciphertext too short".to_owned());
        }
        let nonce = StreamCipher::nonce(self.seq);
        let mut pt = vec![0u8; ciphertext.len() - MAC_BYTES];
        crypto_secretbox_open_easy(&mut pt, ciphertext, &nonce, &self.key)
            .map_err(|e| format!("secretbox open: {e:?}"))?;
        Ok(pt)
    }
}
