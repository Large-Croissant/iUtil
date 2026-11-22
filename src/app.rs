use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

use anyhow::Context;
use eframe::egui;

use crate::{
    backup::BackupSource,
    device::{self, DeviceSummary},
    export::{ExportSummary, Formats, export_all},
    messages::build_bundle,
    model::Progress,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    BackupFolder,
    UsbDevice,
}

enum WorkerMessage {
    Progress(Progress),
    Devices(Vec<DeviceSummary>),
    Done(ExportSummary),
    Failed(String),
}

pub struct IUtilApp {
    source_mode: SourceMode,
    backup_path: String,
    output_path: String,
    password: String,
    show_password: bool,
    formats: Formats,
    devices: Vec<DeviceSummary>,
    selected_udid: Option<String>,
    status: String,
    progress: Option<(f32, String)>,
    worker: Option<Receiver<WorkerMessage>>,
    scanning: bool,
    summary: Option<ExportSummary>,
}

impl IUtilApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let output_path = std::env::current_dir()
            .unwrap_or_default()
            .join("exports")
            .to_string_lossy()
            .to_string();

        Self {
            source_mode: SourceMode::BackupFolder,
            backup_path: String::new(),
            output_path,
            password: String::new(),
            show_password: false,
            formats: Formats {
                json: true,
                csv: true,
                html: true,
            },
            devices: Vec::new(),
            selected_udid: None,
            status: "Choose an iOS backup folder, or connect an iPhone over USB.".to_owned(),
            progress: None,
            worker: None,
            scanning: false,
            summary: None,
        }
    }

    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    fn can_start(&self) -> bool {
        if self.busy()
            || self.output_path.trim().is_empty()
            || !(self.formats.json || self.formats.csv || self.formats.html)
        {
            return false;
        }

        match self.source_mode {
            SourceMode::BackupFolder => !self.backup_path.trim().is_empty(),
            SourceMode::UsbDevice => self.selected_udid.is_some(),
        }
    }

    fn refresh_devices(&mut self) {
        if self.busy() {
            return;
        }

        let (sender, receiver) = mpsc::channel();
        self.worker = Some(receiver);
        self.scanning = true;
        self.progress = None;
        self.status = "Looking for connected iPhones...".to_owned();

        std::thread::spawn(move || match device::list_devices() {
            Ok(devices) => {
                let _ = sender.send(WorkerMessage::Devices(devices));
            }
            Err(error) => {
                let _ = sender.send(WorkerMessage::Failed(format!("{error:#}")));
            }
        });
    }

    fn start_export(&mut self) {
        if !self.can_start() {
            return;
        }

        let mode = self.source_mode;
        let backup_path = self.backup_path.trim().to_owned();
        let output_path = self.output_path.trim().to_owned();
        let udid = self.selected_udid.clone();
        let password = std::mem::take(&mut self.password);
        let formats = self.formats;
        let (sender, receiver) = mpsc::channel();

        self.progress = Some((0.0, "Starting export...".to_owned()));
        self.status = "Exporting...".to_owned();
        self.summary = None;
        self.worker = Some(receiver);
        self.scanning = false;

        std::thread::spawn(move || {
            let send_progress = |progress: Progress| {
                let _ = sender.send(WorkerMessage::Progress(progress));
            };

            let result = (|| -> anyhow::Result<ExportSummary> {
                let password = if password.is_empty() {
                    None
                } else {
                    Some(password.as_str())
                };

                let mut temp_backup: Option<tempfile::TempDir> = None;
                let backup_dir = match mode {
                    SourceMode::BackupFolder => PathBuf::from(&backup_path),
                    SourceMode::UsbDevice => {
                        let udid = udid
                            .as_deref()
                            .context("No iPhone is selected")?;
                        let temp = tempfile::Builder::new()
                            .prefix("iutil-backup-")
                            .tempdir()
                            .context("Unable to create a temporary backup folder")?;
                        device::backup_device(udid, password, temp.path(), &mut |progress| {
                            send_progress(progress)
                        })?;
                        let dir = temp.path().join(udid);
                        temp_backup = Some(temp);
                        dir
                    }
                };

                let source = BackupSource::open(&backup_dir, password)?;
                let mut bundle = build_bundle(&source, &mut |progress| send_progress(progress))?;
                let summary = export_all(
                    &source,
                    &mut bundle,
                    Path::new(&output_path),
                    formats,
                    &mut |progress| send_progress(progress),
                )?;

                drop(source);
                drop(temp_backup);
                Ok(summary)
            })();

            match result {
                Ok(summary) => {
                    let _ = sender.send(WorkerMessage::Done(summary));
                }
                Err(error) => {
                    let _ = sender.send(WorkerMessage::Failed(format!("{error:#}")));
                }
            }
        });
    }

    fn poll_worker(&mut self, ctx: &egui::Context) {
        let Some(receiver) = &self.worker else {
            return;
        };

        loop {
            match receiver.try_recv() {
                Ok(WorkerMessage::Progress(progress)) => {
                    self.progress = Some((progress.fraction, progress.message.clone()));
                    self.status = progress.message;
                }
                Ok(WorkerMessage::Devices(devices)) => {
                    self.status = if devices.is_empty() {
                        "No iPhone found. Connect one with a USB cable, unlock it, and try again."
                            .to_owned()
                    } else {
                        format!(
                            "Found {} iPhone{}. Unlock it and tap Trust if asked.",
                            devices.len(),
                            if devices.len() == 1 { "" } else { "s" }
                        )
                    };

                    if let Some(selected) = &self.selected_udid
                        && !devices.iter().any(|device| &device.udid == selected)
                    {
                        self.selected_udid = None;
                    }
                    if self.selected_udid.is_none() {
                        self.selected_udid =
                            devices.first().map(|device| device.udid.clone());
                    }

                    self.devices = devices;
                    self.progress = None;
                    self.worker = None;
                    self.scanning = false;
                    break;
                }
                Ok(WorkerMessage::Done(summary)) => {
                    self.status = format!(
                        "Exported {} messages from {} conversations ({} attachments copied).",
                        summary.message_count, summary.chat_count, summary.copied_attachments
                    );
                    self.summary = Some(summary);
                    self.progress = None;
                    self.worker = None;
                    break;
                }
                Ok(WorkerMessage::Failed(error)) => {
                    self.status = if self.scanning {
                        format!("Could not find iPhones: {error}")
                    } else {
                        format!("Export failed: {error}")
                    };
                    self.progress = None;
                    self.worker = None;
                    self.scanning = false;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = "The background worker stopped unexpectedly.".to_owned();
                    self.progress = None;
                    self.worker = None;
                    self.scanning = false;
                    break;
                }
            }
        }

        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

impl eframe::App for IUtilApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_worker(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.set_max_width(880.0);
            ui.add_space(18.0);
            ui.heading("iUtil");
            ui.label("Export your iPhone data to open, portable formats.");
            ui.add_space(14.0);

            ui.group(|ui| {
                ui.heading("1. Where are the messages?");
                ui.add_space(6.0);
                ui.radio_value(
                    &mut self.source_mode,
                    SourceMode::BackupFolder,
                    "Existing iOS backup folder",
                );
                if ui
                    .radio_value(
                        &mut self.source_mode,
                        SourceMode::UsbDevice,
                        "Connected iPhone over USB",
                    )
                    .clicked()
                    && self.devices.is_empty()
                    && !self.busy()
                {
                    self.refresh_devices();
                }

                ui.add_space(10.0);
                match self.source_mode {
                    SourceMode::BackupFolder => {
                        ui.horizontal(|ui| {
                            ui.label("Backup folder:");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.backup_path)
                                    .hint_text("C:\\Users\\...\\Apple\\MobileSync\\Backup\\<device>")
                                    .desired_width(460.0),
                            );
                            if ui.button("Browse...").clicked()
                                && let Some(folder) = rfd::FileDialog::new().pick_folder()
                            {
                                self.backup_path = folder.to_string_lossy().to_string();
                            }
                        });
                    }
                    SourceMode::UsbDevice => {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(!self.busy(), egui::Button::new("Find iPhones"))
                                .clicked()
                            {
                                self.refresh_devices();
                            }
                            ui.label(
                                egui::RichText::new(
                                    "Unlock the iPhone and tap Trust when it asks.",
                                )
                                .small()
                                .weak(),
                            );
                        });

                        ui.add_space(6.0);
                        if self.devices.is_empty() {
                            ui.label("No iPhone detected yet.");
                        } else {
                            let devices = self.devices.clone();
                            for device in &devices {
                                let label = if device.paired {
                                    format!(
                                        "iPhone {} ({})",
                                        device.short_udid(),
                                        device.connection
                                    )
                                } else {
                                    format!(
                                        "iPhone {} ({}) - will ask you to trust this computer",
                                        device.short_udid(),
                                        device.connection
                                    )
                                };
                                ui.radio_value(
                                    &mut self.selected_udid,
                                    Some(device.udid.clone()),
                                    label,
                                );
                            }
                        }
                    }
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Backup password:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.password)
                            .password(!self.show_password)
                            .hint_text("Only needed for encrypted backups")
                            .desired_width(260.0),
                    );
                    ui.checkbox(&mut self.show_password, "Show");
                });
                ui.label(
                    egui::RichText::new(match self.source_mode {
                        SourceMode::BackupFolder => {
                            "Passwords are kept in memory for this export only and are never saved."
                        }
                        SourceMode::UsbDevice => {
                            "If encryption is enabled on the iPhone, this password protects the temporary backup and is used to read it. It is never saved."
                        }
                    })
                    .small()
                    .weak(),
                );
            });

            ui.add_space(12.0);

            ui.group(|ui| {
                ui.heading("2. Where should the export go?");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Output folder:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.output_path).desired_width(520.0),
                    );
                    if ui.button("Browse...").clicked()
                        && let Some(folder) = rfd::FileDialog::new().pick_folder()
                    {
                        self.output_path = folder.to_string_lossy().to_string();
                    }
                });

                ui.add_space(8.0);
                ui.label("Formats:");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.formats.json, "JSON (complete data)");
                    ui.checkbox(&mut self.formats.csv, "CSV (related tables)");
                    ui.checkbox(&mut self.formats.html, "HTML (readable archive)");
                });
            });

            ui.add_space(12.0);

            ui.horizontal(|ui| {
                let button = egui::Button::new(if self.busy() {
                    "Working..."
                } else {
                    "Export messages"
                });
                if ui.add_enabled(self.can_start(), button).clicked() {
                    self.start_export();
                }

                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Open output folder"))
                    .clicked()
                {
                    open_folder(Path::new(self.output_path.trim()));
                }
            });

            if let Some((fraction, message)) = &self.progress {
                ui.add_space(10.0);
                ui.add(
                    egui::ProgressBar::new(*fraction)
                        .show_percentage()
                        .text(message.clone()),
                );
            }

            ui.add_space(10.0);
            ui.label(&self.status);

            if let Some(summary) = &self.summary {
                ui.add_space(6.0);
                ui.group(|ui| {
                    ui.label(format!(
                        "Messages: {}   Conversations: {}   Attachments: {} ({} copied)",
                        summary.message_count,
                        summary.chat_count,
                        summary.attachment_count,
                        summary.copied_attachments
                    ));
                    ui.label(
                        egui::RichText::new(format!(
                            "Saved to {}",
                            summary.output_dir.display()
                        ))
                        .small()
                        .weak(),
                    );
                    for file in &summary.files {
                        ui.label(
                            egui::RichText::new(file.display().to_string())
                                .small()
                                .weak(),
                        );
                    }
                });
            }

            ui.add_space(18.0);
            ui.label(
                egui::RichText::new(
                    "iUtil reads a copy of your backup. It does not modify the phone or the backup, and it does not send data anywhere.",
                )
                .small()
                .weak(),
            );
            ui.add_space(18.0);
        });
    }
}

fn open_folder(path: &Path) {
    if path.as_os_str().is_empty() {
        return;
    }

    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }

    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    }
}
