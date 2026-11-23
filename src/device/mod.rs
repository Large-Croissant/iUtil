use std::{
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    pin::Pin,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    Idevice, IdeviceError, IdeviceService,
    pairing_file::PairingFile,
    provider::{IdeviceProvider, UsbmuxdProvider},
    services::{
        lockdown::LockdownClient,
        mobilebackup2::{
            BackupDelegate, BackupProgress, DirEntryInfo, FsBackupDelegate, MobileBackup2Client,
        },
    },
    usbmuxd::{Connection, UsbmuxdAddr, UsbmuxdConnection, UsbmuxdDevice},
};

use crate::model::Progress;

/// One iPhone currently known to Apple's device service.
#[derive(Debug, Clone)]
pub struct DeviceSummary {
    pub udid: String,
    pub connection: String,
    pub paired: bool,
    /// `Some(true)` when the iPhone encrypts its backups, `Some(false)` when
    /// it does not, and `None` when the setting could not be read.
    pub encrypted_backups: Option<bool>,
}

impl DeviceSummary {
    pub fn short_udid(&self) -> String {
        self.udid.chars().take(8).collect()
    }
}

/// Find every iPhone that Apple's device service can currently see.
pub fn list_devices() -> Result<Vec<DeviceSummary>> {
    runtime()?.block_on(list_devices_async())
}

/// Make a full backup of the connected iPhone into `destination`.
///
/// The iPhone decides whether the backup is encrypted. `password` is only used
/// to confirm we can read an encrypted result; it is never sent to the device
/// and is never used to change the iPhone's encryption settings.
///
/// Returns the folder that contains the backup (`destination/<udid>`), which
/// can be opened with [`crate::backup::BackupSource`].
pub fn backup_device(
    udid: &str,
    password: Option<&str>,
    destination: &Path,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<PathBuf> {
    runtime()?.block_on(backup_device_async(
        udid,
        password,
        destination,
        on_progress,
    ))
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Unable to start the device runtime")
}

async fn connect_mux(addr: &UsbmuxdAddr, tag: u32) -> Result<UsbmuxdConnection> {
    addr.connect(tag).await.context(
        "Could not reach Apple's device service. Install iTunes or the Apple Devices app, then \
         reconnect your iPhone.",
    )
}

async fn list_devices_async() -> Result<Vec<DeviceSummary>> {
    let addr = UsbmuxdAddr::default();
    let mut connection = connect_mux(&addr, 1).await?;

    let devices = connection
        .get_devices()
        .await
        .context("Unable to ask Apple's device service for connected iPhones")?;

    let mut summaries = Vec::with_capacity(devices.len());
    for device in devices {
        let pairing = connection.get_pair_record(&device.udid).await.ok();
        let encrypted_backups = match &pairing {
            Some(pairing) => {
                let provider = PairedProvider {
                    inner: device.to_provider(addr.clone(), "iUtil"),
                    pairing: pairing.clone(),
                };
                tokio::time::timeout(
                    Duration::from_secs(3),
                    read_will_encrypt(&provider, pairing),
                )
                .await
                .ok()
                .flatten()
            }
            None => None,
        };

        summaries.push(DeviceSummary {
            udid: device.udid.clone(),
            connection: connection_label(&device.connection_type),
            paired: pairing.is_some(),
            encrypted_backups,
        });
    }

    Ok(summaries)
}

async fn backup_device_async(
    udid: &str,
    password: Option<&str>,
    destination: &Path,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<PathBuf> {
    let password = password.filter(|password| !password.is_empty());

    on_progress(Progress {
        fraction: 0.0,
        message: "Connecting to your iPhone...".to_owned(),
    });

    let addr = UsbmuxdAddr::default();
    let mut connection = connect_mux(&addr, 2).await?;

    let device = connection.get_device(udid).await.with_context(|| {
        format!("The iPhone {udid} is no longer connected. Reconnect it and try again.")
    })?;

    let pairing = ensure_pairing(&addr, &device, on_progress).await?;

    let provider = PairedProvider {
        inner: device.to_provider(addr, "iUtil"),
        pairing: pairing.clone(),
    };

    on_progress(Progress {
        fraction: 0.0,
        message: "Checking this iPhone's backup settings...".to_owned(),
    });

    let encrypted_backups = read_will_encrypt(&provider, &pairing).await;
    if encrypted_backups == Some(true) && password.is_none() {
        bail!(
            "Encrypted backups are turned on for this iPhone. Enter the iPhone's backup \
             password so iUtil can read the backup it makes. iUtil never changes your \
             iPhone's encryption settings."
        );
    }

    on_progress(Progress {
        fraction: 0.0,
        message: "Backing up your iPhone. Keep it connected and unlocked; this can take a while..."
            .to_owned(),
    });

    let mut client = MobileBackup2Client::connect(&provider)
        .await
        .context("Unable to start the iPhone backup service")?;

    // The password is deliberately not part of the backup request: iOS encrypts
    // a backup with the keybag already configured on the device. Sending a
    // password here is unnecessary and could change the device's settings.
    let response = {
        let delegate = ProgressDelegate::new(on_progress);
        client
            .backup_from_path(destination, Some(udid), None, &delegate)
            .await
            .context(
                "The iPhone backup failed. Unlock the iPhone, keep it connected, and try again.",
            )?
    };

    if let Some(response) = &response
        && let Some(error) = response.get("ErrorCode")
    {
        let description = response
            .get("ErrorDescription")
            .and_then(|value| value.as_string())
            .unwrap_or("no description");
        bail!("The iPhone refused the backup: {error:?} ({description})");
    }

    let backup_dir = destination.join(udid);
    let manifest_path = backup_dir.join("Manifest.plist");
    if !manifest_path.is_file() {
        bail!("The iPhone backup finished, but the backup files are missing");
    }

    if backup_is_encrypted(&manifest_path)? && password.is_none() {
        bail!(
            "This iPhone's backup is encrypted. Enter the backup password and try again so \
             iUtil can read it."
        );
    }

    Ok(backup_dir)
}

/// Read the iPhone's "backups are encrypted" setting. Failure is not fatal:
/// the resulting backup's manifest is checked afterwards either way.
async fn read_will_encrypt(provider: &PairedProvider, pairing: &PairingFile) -> Option<bool> {
    let mut lockdown = LockdownClient::connect(provider).await.ok()?;
    lockdown.start_session(pairing).await.ok()?;
    let value = lockdown
        .get_value(Some("WillEncrypt"), Some("com.apple.mobile.backup"))
        .await
        .ok()?;

    value
        .as_boolean()
        .or_else(|| value.as_unsigned_integer().map(|value| value != 0))
}

fn backup_is_encrypted(manifest_path: &Path) -> Result<bool> {
    let manifest = plist::Value::from_file(manifest_path)
        .with_context(|| format!("Unable to read {}", manifest_path.display()))?;

    Ok(manifest
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("IsEncrypted"))
        .and_then(plist::Value::as_boolean)
        .unwrap_or(false))
}

async fn ensure_pairing(
    addr: &UsbmuxdAddr,
    device: &UsbmuxdDevice,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<PairingFile> {
    let mut connection = connect_mux(addr, 3).await?;
    if let Ok(pairing) = connection.get_pair_record(&device.udid).await {
        return Ok(pairing);
    }

    on_progress(Progress {
        fraction: 0.0,
        message: "Waiting for you to trust this computer on the iPhone...".to_owned(),
    });

    let system_buid = connection
        .get_buid()
        .await
        .context("Unable to read the host identifier from Apple's device service")?;
    let host_id = uuid::Uuid::new_v4().to_string();

    let idevice = connection
        .connect_to_device(device.device_id, LockdownClient::LOCKDOWND_PORT, "iUtil")
        .await
        .context("Unable to connect to the iPhone's pairing service")?;
    let mut lockdown = LockdownClient::new(idevice);

    let pairing = tokio::time::timeout(
        Duration::from_secs(180),
        lockdown.pair(host_id, system_buid, Some("iUtil")),
    )
    .await
    .map_err(|_| anyhow!("Timed out waiting for the iPhone. Unlock it, tap Trust, and try again."))?
    .context("Pairing with the iPhone failed. Unlock it and tap Trust when the prompt appears.")?;

    // Cache the record so future runs do not need the trust prompt. Some
    // versions of Apple's device service reject this; the pairing still works
    // for this run because we pass it to the provider directly.
    if let Ok(bytes) = pairing.clone().serialize()
        && let Ok(mut connection) = connect_mux(addr, 4).await
    {
        let _ = connection.save_pair_record(&device.udid, bytes).await;
    }

    Ok(pairing)
}

fn connection_label(connection: &Connection) -> String {
    match connection {
        Connection::Usb => "USB".to_owned(),
        Connection::Network(address) => format!("Wi-Fi ({address})"),
        Connection::Unknown(description) => description.clone(),
    }
}

/// A USB provider that uses a pairing record we already hold, rather than
/// asking usbmuxd for it again.
#[derive(Debug)]
struct PairedProvider {
    inner: UsbmuxdProvider,
    pairing: PairingFile,
}

impl IdeviceProvider for PairedProvider {
    fn connect(
        &self,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<Idevice, IdeviceError>> + Send>> {
        self.inner.connect(port)
    }

    fn label(&self) -> &str {
        self.inner.label()
    }

    fn get_pairing_file(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PairingFile, IdeviceError>> + Send>> {
        let pairing = self.pairing.clone();
        Box::pin(async move { Ok(pairing) })
    }
}

/// Filesystem delegate that forwards the device's backup progress to the UI.
///
/// `BackupDelegate` requires `Sync`, so the callback sits behind a mutex. The
/// device emits progress many times per second; updates are throttled here.
struct ProgressDelegate<'a> {
    inner: FsBackupDelegate,
    state: Mutex<ProgressState<'a>>,
}

struct ProgressState<'a> {
    sink: Box<dyn FnMut(Progress) + Send + 'a>,
    last_emit: Option<Instant>,
    last_fraction: f32,
}

impl<'a> ProgressDelegate<'a> {
    fn new(on_progress: &'a mut (dyn FnMut(Progress) + Send)) -> Self {
        Self {
            inner: FsBackupDelegate,
            state: Mutex::new(ProgressState {
                sink: Box::new(on_progress),
                last_emit: None,
                last_fraction: 0.0,
            }),
        }
    }
}

impl BackupDelegate for ProgressDelegate<'_> {
    fn get_free_disk_space(&self, path: &Path) -> u64 {
        self.inner.get_free_disk_space(path)
    }

    fn open_file_read<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Read + Send>, IdeviceError>> + Send + 'a>> {
        self.inner.open_file_read(path)
    }

    fn create_file_write<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Write + Send>, IdeviceError>> + Send + 'a>>
    {
        self.inner.create_file_write(path)
    }

    fn create_dir_all<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        self.inner.create_dir_all(path)
    }

    fn remove<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        self.inner.remove(path)
    }

    fn rename<'a>(
        &'a self,
        from: &'a Path,
        to: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        self.inner.rename(from, to)
    }

    fn copy<'a>(
        &'a self,
        src: &'a Path,
        dst: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), IdeviceError>> + Send + 'a>> {
        self.inner.copy(src, dst)
    }

    fn exists<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        self.inner.exists(path)
    }

    fn is_dir<'a>(&'a self, path: &'a Path) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        self.inner.is_dir(path)
    }

    fn list_dir<'a>(
        &'a self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<DirEntryInfo>, IdeviceError>> + Send + 'a>> {
        self.inner.list_dir(path)
    }

    fn on_progress(&self, progress: BackupProgress) {
        let fraction = if progress.overall_progress >= 0.0 {
            (progress.overall_progress / 100.0) as f32
        } else if progress.session_bytes_total > 0 {
            progress.session_bytes_done as f32 / progress.session_bytes_total as f32
        } else {
            return;
        };

        let Ok(mut state) = self.state.lock() else {
            return;
        };

        let fraction = fraction.clamp(0.0, 1.0).max(state.last_fraction);
        let now = Instant::now();
        let due = state
            .last_emit
            .is_none_or(|last| now.duration_since(last) >= Duration::from_millis(150));

        if !due && fraction < 1.0 && fraction - state.last_fraction < 0.01 {
            return;
        }

        state.last_emit = Some(now);
        state.last_fraction = fraction;
        (state.sink)(Progress {
            fraction,
            message: "Backing up your iPhone. Keep it connected and unlocked...".to_owned(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(path: &Path, encrypted: Option<bool>) {
        let mut manifest = plist::Dictionary::new();
        if let Some(encrypted) = encrypted {
            manifest.insert("IsEncrypted".to_owned(), plist::Value::Boolean(encrypted));
        }
        plist::Value::Dictionary(manifest)
            .to_file_xml(path)
            .expect("write manifest");
    }

    #[test]
    fn reads_the_manifest_encryption_flag() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("Manifest.plist");

        write_manifest(&path, Some(true));
        assert!(backup_is_encrypted(&path).expect("read encrypted manifest"));

        write_manifest(&path, Some(false));
        assert!(!backup_is_encrypted(&path).expect("read plaintext manifest"));

        write_manifest(&path, None);
        assert!(!backup_is_encrypted(&path).expect("read manifest without a flag"));
    }

    #[test]
    fn progress_never_goes_backwards_and_reaches_the_end() {
        let mut updates = Vec::new();
        {
            let mut sink = |progress: Progress| updates.push(progress.fraction);
            let delegate = ProgressDelegate::new(&mut sink);

            delegate.on_progress(BackupProgress {
                overall_progress: 10.0,
                ..Default::default()
            });
            // A lower estimate must not move the bar backwards.
            delegate.on_progress(BackupProgress {
                overall_progress: 5.0,
                ..Default::default()
            });
            delegate.on_progress(BackupProgress {
                overall_progress: 100.0,
                ..Default::default()
            });
        }

        assert_eq!(updates.len(), 2);
        assert!((updates[0] - 0.1).abs() < 1e-6);
        assert_eq!(updates[1], 1.0);
    }

    #[test]
    fn progress_falls_back_to_session_bytes() {
        let mut updates = Vec::new();
        {
            let mut sink = |progress: Progress| updates.push(progress.fraction);
            let delegate = ProgressDelegate::new(&mut sink);

            delegate.on_progress(BackupProgress {
                overall_progress: -1.0,
                session_bytes_done: 5,
                session_bytes_total: 10,
                ..Default::default()
            });
        }

        assert_eq!(updates, vec![0.5]);
    }
}
