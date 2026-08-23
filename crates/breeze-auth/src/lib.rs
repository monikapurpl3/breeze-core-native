//! Breeze Core's authentication, as the existing clients expect it.
//!
//! Full API access needs **two** credentials: the shared API key, which on its
//! own authorises only enrolment, and a per-device credential. Devices come in
//! two flavours and are pinned to the one they enrolled with:
//!
//! * **v1** — an opaque bearer token; the server stores its SHA-256.
//! * **v2** — an Ed25519 key pair; the server stores only the public half, so a
//!   leak of `devices.json` yields nothing forgeable.
//!
//! Nothing here touches HTTP. A caller lifts headers into [`Presented`] and gets
//! back a [`Decision`], which keeps the whole thing testable and means the
//! ordering guarantees below are properties of a pure function.
//!
//! # Drop-in constraints
//!
//! This must authenticate the *existing* `devices.json` untouched — both
//! versions, no re-pairing. Two parts of that are wire contract rather than
//! implementation choice:
//!
//! * the rejection codes and the `retryable` flag in [`reject`], which the
//!   Android app branches on. They exist because treating a drifted clock as a
//!   dead credential once locked users out for days;
//! * the canonical string in [`signing`], reproduced byte for byte by three
//!   separate clients.

pub mod nonce;
pub mod reject;
pub mod signing;
pub mod verify;

pub use nonce::NonceCache;
pub use reject::{Reason, Rejection, RejectionBody, SkewInfo};
pub use verify::{bearer_from_header, hash_secret, Authenticated, Decision, Presented, Verifier};
