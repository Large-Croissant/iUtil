use std::{
    io::{copy, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use crabapple::{
    Authentication, Backup,
    backup::models::manifest::manifest_plist::ManifestData,
};
use imessage_database::{
    tables::{attachment::Attachment, table::DEFAULT_PATH_IOS},
    util::platform::Platform,
};

use crate::model::{AttachmentRecord, AttachmentSource};

/// An opened iOS backup: either a plain folder or an encrypted backup that is
/// decrypted on demand.
pub struct BackupSource {
    root: PathBuf,
    db_path: PathBuf,
    encrypted: bool,
    device_name: Option<String>,
    product_version: Option<String>,
    backup: Option<Backup>,
    _temp_db: Option<tempfile::TempPath>,
}

impl BackupSource {
    pub fn open(root: impl Into<PathBuf>, password: Option<&str>) -> Result<Self> {
        let root = root.into();
        if !root.is_dir() {
            bail!(
                "The selected path is not a folder: {}",
                root.display()
            );
        }

        let manifest_path = root.join("Manifest.plist");
        if !manifest_path.is_file() {
            bail!(
                "This does not look like an iOS backup (Manifest.plist is missing): {}",
                root.display()
            );
        }

        let manifest = ManifestData::from_plist(&manifest_path)
            .with_context(|| format!("Unable to read {}", manifest_path.display()))?;

        let device_name = Some(manifest.lockdown.device_name.clone());
        let product_version = Some(manifest.lockdown.product_version.clone());

        if manifest.is_encrypted {
            let password = password
                .filter(|password| !password.is_empty())
                .context("This backup is encrypted. Enter the backup password to continue.")?;

            let backup = Backup::open(&root, &Authentication::Password(password.to_owned()))
                .context("Unable to open the encrypted backup. Check the password.")?;

            let (_, file_id) = DEFAULT_PATH_IOS.split_at(3);
            let entry = backup
                .get_file(file_id)
                .context("The backup does not contain the Messages database")?;
            let mut stream = backup
                .decrypt_entry_stream(&entry)
                .context("Unable to decrypt the Messages database")?;

            let mut temp = tempfile::Builder::new()
                .prefix("iutil-sms-")
                .suffix(".db")
                .tempfile()
                .context("Unable to create a temporary decrypted database")?;
            copy(&mut stream, &mut temp)
                .context("Unable to write the temporary decrypted database")?;
            temp.as_file_mut()
                .flush()
                .context("Unable to flush the temporary decrypted database")?;
            let temp_path = temp.into_temp_path();

            Ok(Self {
                root,
                db_path: temp_path.to_path_buf(),
                encrypted: true,
                device_name,
                product_version,
                backup: Some(backup),
                _temp_db: Some(temp_path),
            })
        } else {
            let db_path = root.join(DEFAULT_PATH_IOS);
            if !db_path.is_file() {
                bail!(
                    "This backup does not contain the Messages database. Make a full, unencrypted backup (or provide the password for an encrypted one)."
                );
            }

            Ok(Self {
                root,
                db_path,
                encrypted: false,
                device_name,
                product_version,
                backup: None,
                _temp_db: None,
            })
        }
    }

    pub fn database_path(&self) -> &Path {
        &self.db_path
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypted
    }

    pub fn device_name(&self) -> Option<&str> {
        self.device_name.as_deref()
    }

    pub fn product_version(&self) -> Option<&str> {
        self.product_version.as_deref()
    }

    pub fn read_attachment(&self, record: &AttachmentRecord) -> Result<Vec<u8>> {
        match &record.source {
            Some(AttachmentSource::File(path)) => std::fs::read(path)
                .with_context(|| format!("Unable to read attachment {}", path.display())),
            Some(AttachmentSource::Encrypted(file_id)) => {
                let backup = self
                    .backup
                    .as_ref()
                    .context("Encrypted backup is no longer available")?;
                let entry = backup
                    .get_file(file_id)
                    .with_context(|| format!("Attachment {file_id} is missing from the backup"))?;
                backup
                    .decrypt_entry(&entry)
                    .with_context(|| format!("Unable to decrypt attachment {file_id}"))
            }
            None => bail!("Attachment has no readable source"),
        }
    }
}

/// Resolve an attachment to something the backup can read later.
pub fn attachment_source(
    source: &BackupSource,
    attachment: &Attachment,
) -> Option<AttachmentSource> {
    let resolved = attachment.resolved_attachment_path(
        &Platform::iOS,
        source.root(),
        None,
    )?;

    if source.is_encrypted() {
        Path::new(&resolved)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| AttachmentSource::Encrypted(name.to_owned()))
    } else {
        Some(AttachmentSource::File(PathBuf::from(resolved)))
    }
}
