// SPDX-License-Identifier: GPL-3.0-or-later
//! Update signatures (minisign, Ed25519). Every release's SHA256SUMS is
//! signed in the release workflow; an update is installed only when the
//! signature is valid for one of the two public keys built in here and the
//! checksum matches the download.
//!
//! PRIMARY signs releases (its secret key is the repo secret
//! MINISIGN_SECRET_KEY). BACKUP's secret key is kept offline and is only used
//! if the primary is ever lost or leaked.

use crate::util::Res;
use minisign_verify::{PublicKey, Signature};

const PRIMARY_PUB: &str = include_str!("../packaging/minisign/primary.pub");
const BACKUP_PUB: &str = include_str!("../packaging/minisign/backup.pub");

/// The checksum list and its signature, as published with every release.
pub const SUMS: &str = "SHA256SUMS";
pub const SUMS_SIG: &str = "SHA256SUMS.minisig";

/// The base64 key line of a minisign `.pub` file.
fn key_line(pub_file: &str) -> &str {
    pub_file
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment:"))
        .unwrap_or("")
}

/// The public keys an update may be signed with: primary, then backup.
pub fn trusted_keys() -> Vec<PublicKey> {
    [PRIMARY_PUB, BACKUP_PUB]
        .iter()
        .filter_map(|f| PublicKey::from_base64(key_line(f)).ok())
        .collect()
}

/// Check `data` against a `.minisig` file's text with any of `keys`, and that
/// the signature was made for this release (its trusted comment names `tag`),
/// so an older signed release can't be passed off as a newer one.
pub fn verify(data: &[u8], sig_text: &str, keys: &[PublicKey], tag: &str) -> Res<()> {
    let sig = Signature::decode(sig_text)
        .map_err(|_| "The release's signature file is damaged, so it wasn't installed.")?;
    if !keys.iter().any(|k| k.verify(data, &sig, false).is_ok()) {
        return Err(
            "The release's signature doesn't match StowLock's signing keys, so it wasn't installed."
                .into(),
        );
    }
    if !sig.trusted_comment().split_whitespace().any(|w| w == tag) {
        return Err(format!(
            "The signature was made for a different release (\"{}\"), not {tag}, so it wasn't installed.",
            sig.trusted_comment()
        ));
    }
    Ok(())
}

/// The checksum SHA256SUMS lists for `name` ("<hex>  <name>" lines).
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let mut parts = l.split_whitespace();
        let hex = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && hex.len() == 64 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| hex.to_ascii_lowercase())
    })
}

#[cfg(test)]
pub mod test_keys {
    //! Throwaway keys for tests: sign like the release workflow does.
    use minisign::KeyPair;

    pub struct TestKey {
        pair: KeyPair,
    }

    impl TestKey {
        pub fn new() -> Self {
            TestKey {
                pair: KeyPair::generate_unencrypted_keypair().unwrap(),
            }
        }
        pub fn public(&self) -> minisign_verify::PublicKey {
            minisign_verify::PublicKey::from_base64(&self.pair.pk.to_base64()).unwrap()
        }
        /// Sign `data` with the trusted comment the workflow uses.
        pub fn sign(&self, data: &[u8], trusted_comment: &str) -> String {
            minisign::sign(
                Some(&self.pair.pk),
                &self.pair.sk,
                std::io::Cursor::new(data),
                Some(trusted_comment),
                Some("signature from minisign secret key"),
            )
            .unwrap()
            .into_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_keys::TestKey;
    use super::*;

    #[test]
    fn both_built_in_keys_load() {
        let keys = trusted_keys();
        assert_eq!(keys.len(), 2, "primary and backup");
        assert!(key_line(PRIMARY_PUB).starts_with("RWQcs3jtP2jTiAMa"));
        assert!(key_line(BACKUP_PUB).starts_with("RWRObC9qlP598z1k"));
    }

    #[test]
    fn a_signature_from_either_key_is_accepted() {
        let (primary, backup) = (TestKey::new(), TestKey::new());
        let keys = [primary.public(), backup.public()];
        let sums = b"abc  stowlock-linux-x86_64\n";
        for k in [&primary, &backup] {
            let sig = k.sign(sums, "StowLock v1.2.0");
            assert!(verify(sums, &sig, &keys, "v1.2.0").is_ok());
        }
    }

    #[test]
    fn bad_signatures_are_refused() {
        let (ours, stranger) = (TestKey::new(), TestKey::new());
        let keys = [ours.public()];
        let sums = b"abc  stowlock-linux-x86_64\n";
        // Signed by a key StowLock doesn't trust.
        let e = verify(
            sums,
            &stranger.sign(sums, "StowLock v1.2.0"),
            &keys,
            "v1.2.0",
        );
        assert!(e.unwrap_err().contains("doesn't match"));
        // Changed after signing.
        let sig = ours.sign(sums, "StowLock v1.2.0");
        assert!(verify(b"evil  stowlock-linux-x86_64\n", &sig, &keys, "v1.2.0").is_err());
        // An older release's genuine signature, offered as a newer release.
        let e = verify(sums, &sig, &keys, "v1.3.0");
        assert!(e.unwrap_err().contains("different release"));
        // Garbage.
        assert!(verify(sums, "not a signature", &keys, "v1.2.0").is_err());
    }

    #[test]
    fn reads_checksums_from_the_list() {
        let h = "a".repeat(64);
        let sums = format!(
            "{h}  stowlock-linux-x86_64\n{}  aur-PKGBUILD\n",
            "b".repeat(64)
        );
        assert_eq!(checksum_for(&sums, "stowlock-linux-x86_64"), Some(h));
        assert_eq!(checksum_for(&sums, "missing"), None);
        assert_eq!(
            checksum_for("short  stowlock-linux-x86_64", "stowlock-linux-x86_64"),
            None
        );
    }
}
