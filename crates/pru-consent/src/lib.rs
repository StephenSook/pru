//! Signed consent records for Pru's egress gateway.
//!
//! A consent document maps to one Biscuit token. The token carries the signed
//! record as authority facts and carries its expiry as a Biscuit Datalog check.
//! Revocation remains external state, as required by Biscuit's design.

use std::collections::HashSet;
use std::fmt;
use std::time::SystemTime;

use argon2::Argon2;
use argon2::password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash};
use biscuit_auth::macros::{authorizer, biscuit};
use biscuit_auth::{Algorithm, Biscuit, KeyPair, PublicKey};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The minimum consent PIN length required by Rev. Proc. 2013-14.
pub const MINIMUM_PIN_LENGTH: usize = 5;

/// The action authorized by a consent document.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsentKind {
    /// Use tax return information within the tax return preparer's practice.
    Use,
    /// Disclose tax return information to the named recipient.
    Disclose,
}

impl ConsentKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Use => "use",
            Self::Disclose => "disclose",
        }
    }

    fn parse(value: &str) -> Result<Self, ConsentError> {
        match value {
            "use" => Ok(Self::Use),
            "disclose" => Ok(Self::Disclose),
            other => Err(ConsentError::InvalidRecord(format!(
                "unknown consent kind: {other}"
            ))),
        }
    }
}

impl fmt::Display for ConsentKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A signed consent record based on Rev. Proc. 2013-14.
///
/// `pin_hash` is an Argon2id PHC string with a random salt. The plaintext PIN
/// is accepted only by the constructor and minting operation and is never
/// retained in this record or written to a Biscuit token.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ConsentRecord {
    pub client_id: String,
    pub kind: ConsentKind,
    pub recipient: Option<String>,
    pub purpose: String,
    pub signed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signer_name: String,
    pub pin_hash: String,
}

impl ConsentRecord {
    /// Create a record with the default expiry of one year after signing.
    ///
    /// # Errors
    ///
    /// Returns an error for a short PIN, invalid consent fields, an invalid
    /// recipient-kind combination, or a timestamp outside the supported range.
    pub fn new(
        client_id: impl Into<String>,
        kind: ConsentKind,
        recipient: Option<String>,
        purpose: impl Into<String>,
        signed_at: DateTime<Utc>,
        signer_name: impl Into<String>,
        pin: &str,
    ) -> Result<Self, ConsentError> {
        validate_pin_length(pin)?;
        let expires_at = signed_at
            .checked_add_signed(Duration::days(365))
            .ok_or(ConsentError::TimestampOutOfRange)?;
        let pin_hash = Argon2::default()
            .hash_password(pin.as_bytes())
            .map_err(|error| ConsentError::PinHash(error.to_string()))?
            .to_string();

        let record = Self {
            client_id: client_id.into(),
            kind,
            recipient,
            purpose: purpose.into(),
            signed_at,
            expires_at,
            signer_name: signer_name.into(),
            pin_hash,
        };
        record.validate()?;
        Ok(record)
    }

    /// Override the default expiry while preserving the signed consent facts.
    ///
    /// # Errors
    ///
    /// Returns an error unless the new expiry is later than `signed_at`.
    pub fn with_expiry(mut self, expires_at: DateTime<Utc>) -> Result<Self, ConsentError> {
        self.expires_at = expires_at;
        self.validate()?;
        Ok(self)
    }

    fn validate(&self) -> Result<(), ConsentError> {
        for (field, value) in [
            ("client_id", self.client_id.as_str()),
            ("purpose", self.purpose.as_str()),
            ("signer_name", self.signer_name.as_str()),
            ("pin_hash", self.pin_hash.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ConsentError::InvalidRecord(format!(
                    "{field} must not be empty"
                )));
            }
        }

        if self.expires_at <= self.signed_at {
            return Err(ConsentError::InvalidRecord(
                "expires_at must be later than signed_at".to_owned(),
            ));
        }

        match (&self.kind, &self.recipient) {
            (ConsentKind::Use, None) => Ok(()),
            (ConsentKind::Disclose, Some(recipient)) if !recipient.trim().is_empty() => Ok(()),
            (ConsentKind::Use, Some(_)) => Err(ConsentError::InvalidRecord(
                "use consent must not name a disclosure recipient".to_owned(),
            )),
            (ConsentKind::Disclose, _) => Err(ConsentError::InvalidRecord(
                "disclose consent requires a non-empty recipient".to_owned(),
            )),
        }
    }
}

/// Facts recovered from a verified and authorized consent token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VerifiedConsent {
    pub client_id: String,
    pub kind: ConsentKind,
    pub recipient: Option<String>,
    pub purpose: String,
    pub signed_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub signer_name: String,
    pub pin_hash: String,
    /// Hex-encoded Biscuit authority-block revocation identifier.
    pub revocation_id: String,
}

/// External deny-list for Biscuit authority-block revocation identifiers.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RevocationList {
    revoked: HashSet<String>,
}

impl RevocationList {
    /// Add a verified token's revocation identifier to the deny-list.
    pub fn revoke(&mut self, revocation_id: impl Into<String>) {
        self.revoked.insert(revocation_id.into());
    }

    /// Return true when an identifier has been revoked.
    #[must_use]
    pub fn is_revoked(&self, revocation_id: &str) -> bool {
        self.revoked.contains(revocation_id)
    }
}

/// An Ed25519 Biscuit authority that mints and verifies consent tokens.
#[derive(Debug)]
pub struct ConsentAuthority {
    keypair: KeyPair,
}

/// A public-key-only verifier suitable for the egress gateway.
#[derive(Clone, Copy, Debug)]
pub struct ConsentVerifier {
    public_key: PublicKey,
}

impl ConsentAuthority {
    /// Create an authority with a fresh Ed25519 key pair.
    #[must_use]
    pub fn new() -> Self {
        Self {
            keypair: KeyPair::new_with_algorithm(Algorithm::Ed25519),
        }
    }

    /// Create an authority from an existing Ed25519 key pair.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied key pair is not Ed25519.
    pub fn from_keypair(keypair: KeyPair) -> Result<Self, ConsentError> {
        if !matches!(keypair, KeyPair::Ed25519(_)) {
            return Err(ConsentError::NonEd25519Key);
        }
        Ok(Self { keypair })
    }

    /// Return a verifier that contains only the Ed25519 public key.
    #[must_use]
    pub fn verifier(&self) -> ConsentVerifier {
        ConsentVerifier {
            public_key: self.keypair.public(),
        }
    }

    /// Mint one Biscuit token for one consent document.
    ///
    /// Minting requires the same PIN that produced the salted `pin_hash`.
    ///
    /// # Errors
    ///
    /// Returns an error when the record is invalid, the PIN is wrong, or token
    /// construction or serialization fails.
    pub fn mint(&self, record: &ConsentRecord, pin: &str) -> Result<String, ConsentError> {
        validate_pin_length(pin)?;
        record.validate()?;
        verify_pin(pin, &record.pin_hash)?;

        let client_id = record.client_id.as_str();
        let kind = record.kind.as_str();
        let recipient = record.recipient.as_deref().unwrap_or("");
        let purpose = record.purpose.as_str();
        let signed_at: SystemTime = record.signed_at.into();
        let expires_at: SystemTime = record.expires_at.into();
        let signer_name = record.signer_name.as_str();
        let pin_hash = record.pin_hash.as_str();

        let token = biscuit!(
            r#"
                consent(
                    {client_id},
                    {kind},
                    {recipient},
                    {purpose},
                    {signed_at},
                    {expires_at},
                    {signer_name},
                    {pin_hash}
                );
                check if time($time), $time < {expires_at};
            "#,
        )
        .build(&self.keypair)?;

        Ok(token.to_base64()?)
    }

    /// Verify the signature, revocation state, Datalog expiry check, and record.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid signature or record, a revoked token, an
    /// expired token, or any failed Biscuit authorization check.
    pub fn verify(
        &self,
        encoded_token: &str,
        now: DateTime<Utc>,
        revocations: &RevocationList,
    ) -> Result<VerifiedConsent, ConsentError> {
        self.verifier().verify(encoded_token, now, revocations)
    }
}

impl ConsentVerifier {
    /// Create a verifier from an Ed25519 public key.
    ///
    /// # Errors
    ///
    /// Returns an error when the supplied key is not Ed25519.
    pub fn from_public_key(public_key: PublicKey) -> Result<Self, ConsentError> {
        if !matches!(public_key, PublicKey::Ed25519(_)) {
            return Err(ConsentError::NonEd25519Key);
        }
        Ok(Self { public_key })
    }

    /// Decode an Ed25519 public key from its hexadecimal representation.
    ///
    /// # Errors
    ///
    /// Returns an error when the string is not a valid Ed25519 public key.
    pub fn from_hex_public_key(encoded: &str) -> Result<Self, ConsentError> {
        let public_key = PublicKey::from_bytes_hex(encoded, Algorithm::Ed25519)?;
        Self::from_public_key(public_key)
    }

    /// Return the public key as lowercase hexadecimal bytes.
    #[must_use]
    pub fn public_key_hex(&self) -> String {
        self.public_key.to_bytes_hex()
    }

    /// Verify the signature, revocation state, Datalog expiry check, and record.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid signature or record, a revoked token, an
    /// expired token, or any failed Biscuit authorization check.
    pub fn verify(
        &self,
        encoded_token: &str,
        now: DateTime<Utc>,
        revocations: &RevocationList,
    ) -> Result<VerifiedConsent, ConsentError> {
        let token = Biscuit::from_base64(encoded_token, self.public_key)?;
        let revocation_id = authority_revocation_id(&token)?;
        if revocations.is_revoked(&revocation_id) {
            return Err(ConsentError::Revoked(revocation_id));
        }

        let record = extract_record(&token)?;
        record.validate()?;

        let now_system: SystemTime = now.into();
        let mut authorizer = authorizer!(
            r#"
                time({now_system});
                allow if true;
            "#,
        )
        .build(&token)?;

        if let Err(error) = authorizer.authorize() {
            if now >= record.expires_at {
                return Err(ConsentError::Expired(record.expires_at));
            }
            return Err(error.into());
        }

        Ok(VerifiedConsent {
            client_id: record.client_id,
            kind: record.kind,
            recipient: record.recipient,
            purpose: record.purpose,
            signed_at: record.signed_at,
            expires_at: record.expires_at,
            signer_name: record.signer_name,
            pin_hash: record.pin_hash,
            revocation_id,
        })
    }
}

impl Default for ConsentAuthority {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors returned while constructing, minting, or verifying consent.
#[derive(Debug, Error)]
pub enum ConsentError {
    #[error("PIN must contain at least {MINIMUM_PIN_LENGTH} characters")]
    PinTooShort,
    #[error("PIN does not match the consent record")]
    InvalidPin,
    #[error("invalid consent record: {0}")]
    InvalidRecord(String),
    #[error("consent expired at {0}")]
    Expired(DateTime<Utc>),
    #[error("consent token is revoked: {0}")]
    Revoked(String),
    #[error("the consent authority requires an Ed25519 key pair")]
    NonEd25519Key,
    #[error("timestamp is outside the supported range")]
    TimestampOutOfRange,
    #[error("PIN hash error: {0}")]
    PinHash(String),
    #[error(transparent)]
    KeyFormat(#[from] biscuit_auth::error::Format),
    #[error(transparent)]
    Biscuit(#[from] biscuit_auth::error::Token),
}

fn validate_pin_length(pin: &str) -> Result<(), ConsentError> {
    if pin.chars().count() < MINIMUM_PIN_LENGTH {
        return Err(ConsentError::PinTooShort);
    }
    Ok(())
}

fn verify_pin(pin: &str, encoded_hash: &str) -> Result<(), ConsentError> {
    let parsed_hash = PasswordHash::new(encoded_hash)
        .map_err(|error| ConsentError::PinHash(error.to_string()))?;
    Argon2::default()
        .verify_password(pin.as_bytes(), &parsed_hash)
        .map_err(|_| ConsentError::InvalidPin)
}

fn authority_revocation_id(token: &Biscuit) -> Result<String, ConsentError> {
    token
        .revocation_identifiers()
        .into_iter()
        .next()
        .map(hex::encode)
        .ok_or_else(|| ConsentError::InvalidRecord("token has no revocation identifier".to_owned()))
}

fn extract_record(token: &Biscuit) -> Result<ConsentRecord, ConsentError> {
    type ConsentTuple = (
        String,
        String,
        String,
        String,
        SystemTime,
        SystemTime,
        String,
        String,
    );

    let mut authorizer = token.authorizer()?;
    let (
        client_id,
        kind,
        recipient,
        purpose,
        signed_at,
        expires_at,
        signer_name,
        pin_hash,
    ): ConsentTuple = authorizer.query_exactly_one(
        "record($client_id, $kind, $recipient, $purpose, $signed_at, $expires_at, $signer_name, $pin_hash) <- consent($client_id, $kind, $recipient, $purpose, $signed_at, $expires_at, $signer_name, $pin_hash)",
    )?;

    Ok(ConsentRecord {
        client_id,
        kind: ConsentKind::parse(&kind)?,
        recipient: if recipient.is_empty() {
            None
        } else {
            Some(recipient)
        },
        purpose,
        signed_at: DateTime::<Utc>::from(signed_at),
        expires_at: DateTime::<Utc>::from(expires_at),
        signer_name,
        pin_hash,
    })
}

#[cfg(test)]
mod tests {
    use biscuit_auth::{Algorithm, KeyPair};
    use chrono::{Duration, TimeZone, Utc};

    use super::{
        ConsentAuthority, ConsentError, ConsentKind, ConsentRecord, ConsentVerifier,
        MINIMUM_PIN_LENGTH, RevocationList,
    };

    const PIN: &str = "53179";

    fn signed_at() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0)
            .single()
            .expect("fixed synthetic test timestamp must be valid")
    }

    fn use_record() -> ConsentRecord {
        ConsentRecord::new(
            "synthetic-client-use",
            ConsentKind::Use,
            None,
            "Prepare a synthetic federal return",
            signed_at(),
            "Synthetic Test Signer",
            PIN,
        )
        .expect("synthetic use consent should be valid")
    }

    fn disclose_record() -> ConsentRecord {
        ConsentRecord::new(
            "synthetic-client-disclose",
            ConsentKind::Disclose,
            Some("Synthetic Payroll Processor".to_owned()),
            "Send synthetic wage totals",
            signed_at(),
            "Synthetic Test Signer",
            PIN,
        )
        .expect("synthetic disclosure consent should be valid")
    }

    #[test]
    fn use_consent_round_trips_and_defaults_to_one_year() {
        let authority = ConsentAuthority::new();
        let record = use_record();
        assert_eq!(record.expires_at, record.signed_at + Duration::days(365));

        let token = authority
            .mint(&record, PIN)
            .expect("minting should succeed");
        let verifier = authority.verifier();
        let public_key_hex = verifier.public_key_hex();
        let public_only_verifier = ConsentVerifier::from_hex_public_key(&public_key_hex)
            .expect("authority must export a valid Ed25519 public key");
        let verified = public_only_verifier
            .verify(
                &token,
                record.signed_at + Duration::days(1),
                &RevocationList::default(),
            )
            .expect("valid use consent should verify");

        assert_eq!(verified.client_id, record.client_id);
        assert_eq!(verified.kind, ConsentKind::Use);
        assert_eq!(verified.recipient, None);
        assert_eq!(verified.purpose, record.purpose);
        assert_eq!(verified.pin_hash, record.pin_hash);
        assert!(!verified.revocation_id.is_empty());

        let wrong_verifier = ConsentAuthority::new().verifier();
        let error = wrong_verifier
            .verify(
                &token,
                record.signed_at + Duration::days(1),
                &RevocationList::default(),
            )
            .expect_err("a different public key must not verify the signature");
        assert!(matches!(error, ConsentError::Biscuit(_)));
    }

    #[test]
    fn disclose_consent_preserves_the_named_recipient() {
        let authority = ConsentAuthority::new();
        let record = disclose_record();
        let token = authority
            .mint(&record, PIN)
            .expect("minting should succeed");
        let verified = authority
            .verify(
                &token,
                record.signed_at + Duration::days(1),
                &RevocationList::default(),
            )
            .expect("valid disclose consent should verify");

        assert_eq!(verified.kind, ConsentKind::Disclose);
        assert_eq!(
            verified.recipient.as_deref(),
            Some("Synthetic Payroll Processor")
        );
    }

    #[test]
    fn recipient_rules_reject_mismatched_consent_kinds() {
        let use_with_recipient = ConsentRecord::new(
            "synthetic-client",
            ConsentKind::Use,
            Some("recipient-not-allowed".to_owned()),
            "Synthetic purpose",
            signed_at(),
            "Synthetic Test Signer",
            PIN,
        );
        assert!(matches!(
            use_with_recipient,
            Err(ConsentError::InvalidRecord(_))
        ));

        let disclose_without_recipient = ConsentRecord::new(
            "synthetic-client",
            ConsentKind::Disclose,
            None,
            "Synthetic purpose",
            signed_at(),
            "Synthetic Test Signer",
            PIN,
        );
        assert!(matches!(
            disclose_without_recipient,
            Err(ConsentError::InvalidRecord(_))
        ));
    }

    #[test]
    fn expiry_is_enforced_by_the_biscuit_datalog_check() {
        let authority = ConsentAuthority::new();
        let record = use_record()
            .with_expiry(signed_at() + Duration::hours(1))
            .expect("test expiry should be valid");
        let token = authority
            .mint(&record, PIN)
            .expect("minting should succeed");

        let error = authority
            .verify(&token, record.expires_at, &RevocationList::default())
            .expect_err("consent must be expired at expires_at");
        assert!(matches!(error, ConsentError::Expired(_)));
    }

    #[test]
    fn revoked_authority_block_is_rejected() {
        let authority = ConsentAuthority::new();
        let record = use_record();
        let token = authority
            .mint(&record, PIN)
            .expect("minting should succeed");
        let verified = authority
            .verify(
                &token,
                record.signed_at + Duration::days(1),
                &RevocationList::default(),
            )
            .expect("token should initially verify");

        let mut revocations = RevocationList::default();
        revocations.revoke(verified.revocation_id.clone());
        let error = authority
            .verify(&token, record.signed_at + Duration::days(1), &revocations)
            .expect_err("revoked consent must be denied");
        assert!(matches!(error, ConsentError::Revoked(id) if id == verified.revocation_id));
    }

    #[test]
    fn pin_is_minimum_five_characters_and_only_its_salted_hash_is_stored() {
        let too_short = "x".repeat(MINIMUM_PIN_LENGTH - 1);
        let error = ConsentRecord::new(
            "synthetic-client",
            ConsentKind::Use,
            None,
            "Synthetic purpose",
            signed_at(),
            "Synthetic Test Signer",
            &too_short,
        )
        .expect_err("short PIN must be rejected");
        assert!(matches!(error, ConsentError::PinTooShort));

        let first = use_record();
        let second = use_record();
        assert_ne!(first.pin_hash, PIN);
        assert_ne!(
            first.pin_hash, second.pin_hash,
            "hashes must use random salts"
        );
        assert!(first.pin_hash.starts_with("$argon2id$"));

        let authority = ConsentAuthority::new();
        let error = authority
            .mint(&first, "wrong")
            .expect_err("wrong PIN must not mint a token");
        assert!(matches!(error, ConsentError::InvalidPin));
    }

    #[test]
    fn mint_revalidates_records_that_callers_can_deserialize_or_modify() {
        let authority = ConsentAuthority::new();
        let mut record = use_record();
        record.kind = ConsentKind::Disclose;
        let error = authority
            .mint(&record, PIN)
            .expect_err("modified invalid record must not mint");
        assert!(matches!(error, ConsentError::InvalidRecord(_)));
    }

    #[test]
    fn p256_keypairs_are_rejected() {
        let p256 = KeyPair::new_with_algorithm(Algorithm::Secp256r1);
        let error =
            ConsentAuthority::from_keypair(p256).expect_err("consent authority must use Ed25519");
        assert!(matches!(error, ConsentError::NonEd25519Key));
    }
}
