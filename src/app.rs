use std::{
    path::Path,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

use eframe::egui;

use crate::{
    backup::BackupSource,
    export::{ExportSummary, Formats, export_all},
    messages::{Progress, build_bundle},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceMode {
    BackupFolder,
    UsbDevice,
}

enum WorkerMessage {
    Progress(Progress),
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
    status: String,
    progress: Option<(f32, String)>,
    worker: Option<Receiver<WorkerMessage>>,
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
            status: "Choose an iOS backup folder to begin.".to_owned(),
            progress: None,
            worker: None,
            summary: None,
        }
    }

    fn busy(&self) -> bool {
        self.worker.is_some()
    }

    fn can_start(&self) -> bool {
        !self.busy()
            && self.source_mode == SourceMode::BackupFolder
            && !self.backup_path.trim().is_empty()
            && !self.output_path.trim().is_empty()
            && (self.formats.json || self.formats.csv || self.formats.html)
    }

    fn start_export(&mut self) {
        if !self.can_start() {
            return;
        }

        let backup_path = self.backup_path.trim().to_owned();
        let output_path = self.output_path.trim().to_owned();
        let password = std::mem::take(&mut self.password);
        let formats = self.formats;
        let (sender, receiver) = mpsc::channel();

        self.progress = Some((0.0, "Starting export...".to_owned()));
        self.status = "Exporting...".to_owned();
        self.summary = None;
        self.worker = Some(receiver);

        std::thread::spawn(move || {
            let send_progress = |progress: Progress| {
                let _ = sender.send(WorkerMessage::Progress(progress));
            };

            let result = (|| -> anyhow::Result<ExportSummary> {
                let source = BackupSource::open(
                    &backup_path,
                    if password.is_empty() {
                        None
                    } else {
                        Some(password.as_str())
                    },
                )?;
                let mut bundle = build_bundle(&source, &mut |progress| send_progress(progress))?;
                export_all(
                    &source,
                    &mut bundle,
                    Path::new(&output_path),
                    formats,
                    &mut |progress| send_progress(progress),
                )
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
                    self.status = format!("Export failed: {error}");
                    self.progress = None;
                    self.worker = None;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.status = "The export worker stopped unexpectedly.".to_owned();
                    self.progress = None;
                    self.worker = None;
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
                ui.radio_value(
                    &mut self.source_mode,
                    SourceMode::UsbDevice,
                    "Connected iPhone over USB (coming next)",
                );

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
                        ui.label(
                            "USB backup support is being added. For now, make a backup with Apple Devices or iTunes and select it above.",
                        );
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
                    egui::RichText::new(
                        "Passwords are kept in memory for this export only and are never saved.",
                    )
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
                        egui::TextEdit::singleline(&mut self.output_path)
                            .desired_width(520.0),
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
                    "Exporting..."
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
