use anyhow::{Context, Result, ensure};
use rcgen::{CertifiedKey, generate_simple_self_signed};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::{ZeroizeOnDrop, Zeroizing};

pub struct Identity {
    pub certificate: CertificateDer<'static>,
    key: Zeroizing<Vec<u8>>,
    pub fingerprint: String,
}
#[derive(Serialize, Deserialize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct StoredIdentity {
    version: u16,
    certificate: String,
    protected_key: String,
}

pub fn fingerprint(cert: &[u8]) -> String {
    hex::encode(Sha256::digest(cert))
}
pub fn pretty_fingerprint(value: &str) -> String {
    value
        .as_bytes()
        .chunks(4)
        .map(|p| String::from_utf8_lossy(p).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "State directory must not be a symlink"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
/// Keep this file open for the lifetime of the worker. Never unlink a lock file:
/// a second inode at the same path would permit a second owner.
pub fn lock_directory(directory: &Path) -> Result<fs::File> {
    private_directory(directory)?;
    let path = directory.join("instance.lock");
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            read_private(&path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    file.try_lock().map_err(|error| {
        anyhow::anyhow!(
            "Cannot lock Ravel state directory (another instance may be running): {error}"
        )
    })?;
    Ok(file)
}

pub fn read_private(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink() && metadata.len() <= 65536,
        "Invalid state file"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "State file must be private (chmod 600)"
        );
    }
    Ok(fs::read(path)?)
}
pub fn atomic_private(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    let parent = path.parent().context("State file needs a directory")?;
    private_directory(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    if overwrite {
        file.persist(path).map_err(|e| e.error)?;
    } else {
        file.persist_noclobber(path).map_err(|e| e.error)?;
    }
    Ok(())
}

impl Identity {
    pub fn load_or_create(directory: &Path) -> Result<Self> {
        private_directory(directory)?;
        let path = directory.join("identity.json");
        if !path.try_exists()? {
            let CertifiedKey { cert, signing_key } =
                generate_simple_self_signed(vec!["ravel.local".into()])?;
            let key = Zeroizing::new(signing_key.serialize_der());
            let protected = Zeroizing::new(protect(&key)?);
            let stored = StoredIdentity {
                version: 1,
                certificate: hex::encode(cert.der()),
                protected_key: hex::encode(protected.as_slice()),
            };
            let encoded = Zeroizing::new(serde_json::to_vec(&stored)?);
            if let Err(error) = atomic_private(&path, &encoded, false)
                && !path.try_exists()?
            {
                return Err(error);
            }
        }
        let bytes = Zeroizing::new(
            read_private(&path).context("Cannot read identity; identity was not regenerated")?,
        );
        let stored: StoredIdentity = serde_json::from_slice(&bytes)?;
        ensure!(stored.version == 1, "Unknown identity format");
        let cert = hex::decode(&stored.certificate)?;
        ensure!(
            !cert.is_empty() && cert.len() <= 2048,
            "Invalid identity certificate"
        );
        let protected = Zeroizing::new(hex::decode(&stored.protected_key)?);
        let key = Zeroizing::new(unprotect(&protected)?);
        // Parsing the key now makes corruption a startup failure.
        rustls::crypto::ring::sign::any_supported_type(&PrivateKeyDer::Pkcs8(
            PrivatePkcs8KeyDer::from(key.to_vec()),
        ))?;
        Ok(Self {
            fingerprint: fingerprint(&cert),
            certificate: CertificateDer::from(cert),
            key,
        })
    }
    pub fn private_key(&self) -> PrivateKeyDer<'static> {
        PrivatePkcs8KeyDer::from(self.key.to_vec()).into()
    }
}

pub fn default_directory() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("org", "Ravel", "Ravel")
        .context("No per-user data directory")?
        .data_local_dir()
        .to_owned())
}

#[cfg(not(windows))]
fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(bytes.to_vec())
}
#[cfg(not(windows))]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    Ok(bytes.to_vec())
}
#[cfg(windows)]
fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, true)
}
#[cfg(windows)]
fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    dpapi(bytes, false)
}
#[cfg(windows)]
fn dpapi(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    use windows::Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len().try_into()?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI allocates the output; it must be copied before LocalFree.
    unsafe {
        if encrypt {
            CryptProtectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        } else {
            CryptUnprotectData(
                &input,
                None,
                None,
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )?;
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        zeroize::Zeroize::zeroize(std::slice::from_raw_parts_mut(
            output.pbData,
            output.cbData as usize,
        ));
        let _ = LocalFree(Some(HLOCAL(output.pbData.cast())));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_is_stable_and_corruption_never_rotates_it() {
        let dir = tempfile::tempdir().unwrap();
        let first = Identity::load_or_create(dir.path()).unwrap();
        let again = Identity::load_or_create(dir.path()).unwrap();
        assert_eq!(first.fingerprint, again.fingerprint);
        fs::write(dir.path().join("identity.json"), b"broken").unwrap();
        assert!(Identity::load_or_create(dir.path()).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn refuses_public_state_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        Identity::load_or_create(dir.path()).unwrap();
        fs::set_permissions(
            dir.path().join("identity.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert!(Identity::load_or_create(dir.path()).is_err());
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;
    #[test]
    fn only_one_owner_per_state_directory_and_release_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let guard = lock_directory(dir.path()).unwrap();
        assert!(lock_directory(dir.path()).is_err());
        let _other_guard = lock_directory(other.path()).unwrap();
        drop(guard);
        let _restarted = lock_directory(dir.path()).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn lock_file_is_private_and_symlinks_are_refused() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let guard = lock_directory(dir.path()).unwrap();
        assert_eq!(
            guard.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(guard);
        fs::remove_file(dir.path().join("instance.lock")).unwrap();
        symlink("elsewhere", dir.path().join("instance.lock")).unwrap();
        assert!(lock_directory(dir.path()).is_err());
    }
}
