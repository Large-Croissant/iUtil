use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use idevice::{
    Idevice, IdeviceError, IdeviceService,
    pairing_file::PairingFile,
    provider::{IdeviceProvider, UsbmuxdProvider},
    services::{
        lockdown::LockdownClient,
        mobilebackup2::{FsBackupDelegate, MobileBackup2Client},
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
/// Returns the folder that contains the backup (`destination/<udid>`), which
/// can be opened with [`crate::backup::BackupSource`].
pub fn backup_device(
    udid: &str,
    password: Option<&str>,
    destination: &Path,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    runtime()?.block_on(backup_device_async(udid, password, destination, on_progress))
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
        let paired = connection.get_pair_record(&device.udid).await.is_ok();
        summaries.push(DeviceSummary {
            udid: device.udid,
            connection: connection_label(&device.connection_type),
            paired,
        });
    }

    Ok(summaries)
}

async fn backup_device_async(
    udid: &str,
    password: Option<&str>,
    destination: &Path,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf> {
    on_progress(Progress {
        fraction: 0.01,
        message: "Connecting to your iPhone...".to_owned(),
    });

    let addr = UsbmuxdAddr::default();
    let mut connection = connect_mux(&addr, 2).await?;

    let device = connection.get_device(udid).await.with_context(|| {
        format!(
            "The iPhone {udid} is no longer connected. Reconnect it and try again."
        )
    })?;

    let pairing = ensure_pairing(&addr, &device, on_progress).await?;

    let provider = PairedProvider {
        inner: device.to_provider(addr, "iUtil"),
        pairing,
    };

    on_progress(Progress {
        fraction: 0.02,
        message: "Preparing the backup...".to_owned(),
    });

    let mut client = MobileBackup2Client::connect(&provider)
        .await
        .context("Unable to start the iPhone backup service")?;

    let mut options = plist::Dictionary::new();
    if let Some(password) = password.filter(|password| !password.is_empty()) {
        options.insert(
            "Password".to_owned(),
            plist::Value::String(password.to_owned()),
        );
    }

    on_progress(Progress {
        fraction: 0.03,
        message:
            "Backing up your iPhone. Keep it connected and unlocked; this can take a while..."
                .to_owned(),
    });

    let response = client
        .backup_from_path(destination, Some(udid), Some(options), &FsBackupDelegate)
        .await
        .context(
            "The iPhone backup failed. Unlock the iPhone, keep it connected, and try again.",
        )?;

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
    if !backup_dir.join("Manifest.plist").is_file() {
        bail!("The iPhone backup finished, but the backup files are missing");
    }

    Ok(backup_dir)
}

async fn ensure_pairing(
    addr: &UsbmuxdAddr,
    device: &UsbmuxdDevice,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PairingFile> {
    let mut connection = connect_mux(addr, 3).await?;
    if let Ok(pairing) = connection.get_pair_record(&device.udid).await {
        return Ok(pairing);
    }

    on_progress(Progress {
        fraction: 0.01,
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
    .map_err(|_| {
        anyhow!(
            "Timed out waiting for the iPhone. Unlock it, tap Trust, and try again."
        )
    })?
    .context(
        "Pairing with the iPhone failed. Unlock it and tap Trust when the prompt appears.",
    )?;

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
