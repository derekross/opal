//! Turning whatever the user pastes into a key pair.

use nostr::key::Keys;
use nostr::nips::nip06::FromMnemonic;
use nostr::nips::nip19::FromBech32;
use nostr::nips::nip49::EncryptedSecretKey;

use crate::{Error, Result};

/// What kind of secret the user pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretFormat {
    /// `nsec1…` or 64 hex characters.
    Plain,
    /// NIP-49 `ncryptsec1…`; needs its own password.
    Ncryptsec,
    /// NIP-06 BIP-39 recovery phrase.
    Mnemonic,
}

pub fn detect(input: &str) -> Result<SecretFormat> {
    let input = input.trim();
    if input.starts_with("ncryptsec1") {
        Ok(SecretFormat::Ncryptsec)
    } else if input.starts_with("nsec1")
        || (input.len() == 64 && input.chars().all(|c| c.is_ascii_hexdigit()))
    {
        Ok(SecretFormat::Plain)
    } else if matches!(input.split_whitespace().count(), 12 | 15 | 18 | 21 | 24) {
        Ok(SecretFormat::Mnemonic)
    } else {
        Err(Error::Invalid(
            "expected an nsec, hex key, ncryptsec or recovery phrase".into(),
        ))
    }
}

/// Options that only some formats use.
#[derive(Debug, Default, Clone, Copy)]
pub struct ImportOptions<'a> {
    /// Password protecting an `ncryptsec`.
    pub ncryptsec_password: Option<&'a str>,
    /// NIP-06 account index (defaults to 0).
    pub account_index: Option<u32>,
    /// Optional BIP-39 passphrase ("25th word").
    pub mnemonic_passphrase: Option<&'a str>,
}

pub fn parse_secret(input: &str, opts: ImportOptions<'_>) -> Result<Keys> {
    let input = input.trim();
    match detect(input)? {
        SecretFormat::Plain => Keys::parse(input).map_err(Error::nostr),
        SecretFormat::Ncryptsec => {
            let password = opts
                .ncryptsec_password
                .ok_or_else(|| Error::Invalid("this ncryptsec needs its password".into()))?;
            let encrypted = EncryptedSecretKey::from_bech32(input).map_err(Error::nostr)?;
            let secret = encrypted
                .decrypt(password)
                .map_err(|_| Error::BadPassphrase)?;
            Ok(Keys::new(secret))
        }
        SecretFormat::Mnemonic => {
            let words = input.split_whitespace().collect::<Vec<_>>().join(" ");
            Keys::from_mnemonic_with_account(
                words.as_str(),
                opts.mnemonic_passphrase,
                opts.account_index,
            )
            .map_err(Error::nostr)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::nips::nip19::ToBech32;
    use nostr::nips::nip49::KeySecurity;

    #[test]
    fn nsec_and_hex() {
        let keys = Keys::generate();
        let nsec = keys.secret_key().to_bech32().unwrap();
        let hex = keys.secret_key().to_secret_hex();
        let opts = ImportOptions::default();
        assert_eq!(
            parse_secret(&nsec, opts).unwrap().public_key(),
            keys.public_key()
        );
        assert_eq!(
            parse_secret(&hex, opts).unwrap().public_key(),
            keys.public_key()
        );
    }

    #[test]
    fn ncryptsec() {
        let keys = Keys::generate();
        let enc = EncryptedSecretKey::new(keys.secret_key(), "hunter2", 8, KeySecurity::Medium)
            .unwrap()
            .to_bech32()
            .unwrap();
        let wrong = ImportOptions {
            ncryptsec_password: Some("nope"),
            ..Default::default()
        };
        assert!(matches!(
            parse_secret(&enc, wrong),
            Err(Error::BadPassphrase)
        ));
        let right = ImportOptions {
            ncryptsec_password: Some("hunter2"),
            ..Default::default()
        };
        assert_eq!(
            parse_secret(&enc, right).unwrap().public_key(),
            keys.public_key()
        );
    }

    /// Test vector from NIP-06.
    #[test]
    fn mnemonic_nip06_vector() {
        let words =
            "leader monkey parrot ring guide accident before fence cannon height naive bean";
        let keys = parse_secret(words, ImportOptions::default()).unwrap();
        assert_eq!(
            keys.secret_key().to_secret_hex(),
            "7f7ff03d123792d6ac594bfa67bf6d0c0ab55b6b1fdb6249303fe861f1ccba9a"
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(parse_secret("hello", ImportOptions::default()).is_err());
    }
}
