use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};

use crate::{
    backup::BackupSource,
    model::{AttachmentRecord, ChatRecord, ExportBundle, MessageRecord, Progress},
};

#[derive(Debug, Clone, Copy)]
pub struct Formats {
    pub json: bool,
    pub csv: bool,
    pub html: bool,
}

#[derive(Debug, Clone)]
pub struct ExportSummary {
    pub output_dir: PathBuf,
    pub message_count: usize,
    pub chat_count: usize,
    pub attachment_count: usize,
    pub copied_attachments: usize,
    pub files: Vec<PathBuf>,
}

pub fn export_all(
    source: &BackupSource,
    bundle: &mut ExportBundle,
    output_dir: &Path,
    formats: Formats,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<ExportSummary> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("Unable to create {}", output_dir.display()))?;

    on_progress(Progress {
        fraction: 0.4,
        message: "Copying attachments...".to_owned(),
    });

    let copied_attachments =
        copy_attachments(source, &mut bundle.attachments, output_dir, on_progress)?;

    let mut files = Vec::new();

    if formats.json {
        on_progress(Progress {
            fraction: 0.65,
            message: "Writing messages.json...".to_owned(),
        });
        let path = output_dir.join("messages.json");
        let file = File::create(&path).context("Unable to create messages.json")?;
        serde_json::to_writer_pretty(BufWriter::new(file), bundle)
            .context("Unable to write messages.json")?;
        files.push(path);
    }

    if formats.csv {
        on_progress(Progress {
            fraction: 0.75,
            message: "Writing CSV files...".to_owned(),
        });
        write_csv(bundle, output_dir, &mut files)?;
    }

    if formats.html {
        on_progress(Progress {
            fraction: 0.9,
            message: "Writing the HTML archive...".to_owned(),
        });
        let path = output_dir.join("index.html");
        write_html(bundle, &path)?;
        files.push(path);
    }

    Ok(ExportSummary {
        output_dir: output_dir.to_path_buf(),
        message_count: bundle.source.message_count,
        chat_count: bundle.source.chat_count,
        attachment_count: bundle.attachments.len(),
        copied_attachments,
        files,
    })
}

fn copy_attachments(
    source: &BackupSource,
    attachments: &mut [AttachmentRecord],
    output_dir: &Path,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<usize> {
    if attachments.is_empty() {
        return Ok(0);
    }

    let attachment_dir = output_dir.join("attachments");
    fs::create_dir_all(&attachment_dir).context("Unable to create the attachments folder")?;

    let total = attachments.len();
    let mut copied = 0usize;

    for (index, attachment) in attachments.iter_mut().enumerate() {
        let Some(_) = attachment.source else {
            continue;
        };

        let name = attachment_name(attachment);
        let relative = PathBuf::from("attachments").join(&name);
        let target = output_dir.join(&relative);

        match source.read_attachment(attachment) {
            Ok(bytes) => match fs::write(&target, bytes) {
                Ok(()) => {
                    attachment.export_path = Some(relative.to_string_lossy().replace('\\', "/"));
                    attachment.copied = true;
                    copied += 1;
                }
                Err(error) => {
                    attachment.error = Some(format!("Unable to write attachment: {error}"));
                }
            },
            Err(error) => {
                attachment.error = Some(format!("{error:#}"));
            }
        }

        if index % 25 == 0 || index + 1 == total {
            on_progress(Progress {
                fraction: 0.4 + (index as f32 / total as f32) * 0.25,
                message: format!("Copied {copied} of {total} attachments..."),
            });
        }
    }

    Ok(copied)
}

fn attachment_name(attachment: &AttachmentRecord) -> String {
    let raw = attachment
        .transfer_name
        .as_deref()
        .or_else(|| {
            attachment
                .filename
                .as_deref()
                .and_then(|name| Path::new(name).file_name().and_then(|name| name.to_str()))
        })
        .unwrap_or("attachment");

    let mut safe = String::with_capacity(raw.len());
    for character in raw.chars() {
        if character.is_alphanumeric() || matches!(character, '.' | '-' | '_' | ' ') {
            safe.push(character);
        } else {
            safe.push('_');
        }
    }
    let safe = safe.trim_matches([' ', '.']).trim();
    let safe = if safe.is_empty() { "attachment" } else { safe };

    format!("{}-{safe}", attachment.rowid)
}

fn write_csv(bundle: &ExportBundle, output_dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let messages_path = output_dir.join("messages.csv");
    let mut writer = csv::Writer::from_path(&messages_path)?;
    writer.write_record([
        "rowid",
        "guid",
        "chat_id",
        "sender",
        "is_from_me",
        "service",
        "text",
        "subject",
        "date",
        "date_delivered",
        "date_read",
        "date_edited",
        "item_type",
        "group_title",
        "group_action_type",
        "associated_message_guid",
        "associated_message_type",
        "associated_message_emoji",
        "balloon_bundle_id",
        "expressive_send_style_id",
        "thread_originator_guid",
        "thread_originator_part",
        "num_attachments",
        "num_replies",
        "deleted_from",
        "filter_action",
        "filter_sub_action",
    ])?;
    for message in &bundle.messages {
        writer.write_record([
            message.rowid.to_string(),
            message.guid.clone(),
            message.chat_id.map(|id| id.to_string()).unwrap_or_default(),
            message.sender.clone().unwrap_or_default(),
            message.is_from_me.to_string(),
            message.service.clone().unwrap_or_default(),
            message.text.clone().unwrap_or_default(),
            message.subject.clone().unwrap_or_default(),
            message.date.clone().unwrap_or_default(),
            message.date_delivered.clone().unwrap_or_default(),
            message.date_read.clone().unwrap_or_default(),
            message.date_edited.clone().unwrap_or_default(),
            message.item_type.to_string(),
            message.group_title.clone().unwrap_or_default(),
            message.group_action_type.to_string(),
            message.associated_message_guid.clone().unwrap_or_default(),
            message
                .associated_message_type
                .map(|value| value.to_string())
                .unwrap_or_default(),
            message.associated_message_emoji.clone().unwrap_or_default(),
            message.balloon_bundle_id.clone().unwrap_or_default(),
            message.expressive_send_style_id.clone().unwrap_or_default(),
            message.thread_originator_guid.clone().unwrap_or_default(),
            message.thread_originator_part.clone().unwrap_or_default(),
            message.num_attachments.to_string(),
            message.num_replies.to_string(),
            message
                .deleted_from
                .map(|value| value.to_string())
                .unwrap_or_default(),
            message
                .filter_action
                .map(|value| value.to_string())
                .unwrap_or_default(),
            message
                .filter_sub_action
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ])?;
    }
    writer.flush()?;
    files.push(messages_path);

    let chats_path = output_dir.join("chats.csv");
    let mut writer = csv::Writer::from_path(&chats_path)?;
    writer.write_record([
        "id",
        "identifier",
        "name",
        "display_name",
        "service",
        "participants",
        "message_count",
    ])?;
    for chat in &bundle.chats {
        writer.write_record([
            chat.id.to_string(),
            chat.identifier.clone(),
            chat.name.clone(),
            chat.display_name.clone().unwrap_or_default(),
            chat.service.clone().unwrap_or_default(),
            chat.participants.join("; "),
            chat.message_count.to_string(),
        ])?;
    }
    writer.flush()?;
    files.push(chats_path);

    let participants_path = output_dir.join("participants.csv");
    let mut writer = csv::Writer::from_path(&participants_path)?;
    writer.write_record(["handle_id", "address", "person_centric_id", "chats"])?;
    for participant in &bundle.participants {
        writer.write_record([
            participant.handle_id.to_string(),
            participant.address.clone(),
            participant.person_centric_id.clone().unwrap_or_default(),
            participant
                .chats
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        ])?;
    }
    writer.flush()?;
    files.push(participants_path);

    let attachments_path = output_dir.join("attachments.csv");
    let mut writer = csv::Writer::from_path(&attachments_path)?;
    writer.write_record([
        "message_guid",
        "rowid",
        "guid",
        "filename",
        "transfer_name",
        "uti",
        "mime_type",
        "total_bytes",
        "is_sticker",
        "hide_attachment",
        "emoji_description",
        "export_path",
        "copied",
        "error",
    ])?;
    for attachment in &bundle.attachments {
        writer.write_record([
            attachment.message_guid.clone(),
            attachment.rowid.to_string(),
            attachment.guid.clone().unwrap_or_default(),
            attachment.filename.clone().unwrap_or_default(),
            attachment.transfer_name.clone().unwrap_or_default(),
            attachment.uti.clone().unwrap_or_default(),
            attachment.mime_type.clone().unwrap_or_default(),
            attachment.total_bytes.to_string(),
            attachment.is_sticker.to_string(),
            attachment.hide_attachment.to_string(),
            attachment.emoji_description.clone().unwrap_or_default(),
            attachment.export_path.clone().unwrap_or_default(),
            attachment.copied.to_string(),
            attachment.error.clone().unwrap_or_default(),
        ])?;
    }
    writer.flush()?;
    files.push(attachments_path);

    let reactions_path = output_dir.join("reactions.csv");
    let mut writer = csv::Writer::from_path(&reactions_path)?;
    writer.write_record([
        "message_guid",
        "target_guid",
        "reaction_type",
        "reaction",
        "emoji",
        "sender",
        "date",
        "is_from_me",
    ])?;
    for reaction in &bundle.reactions {
        writer.write_record([
            reaction.message_guid.clone(),
            reaction.target_guid.clone().unwrap_or_default(),
            reaction
                .reaction_type
                .map(|value| value.to_string())
                .unwrap_or_default(),
            reaction.reaction.clone().unwrap_or_default(),
            reaction.emoji.clone().unwrap_or_default(),
            reaction.sender.clone().unwrap_or_default(),
            reaction.date.clone().unwrap_or_default(),
            reaction.is_from_me.to_string(),
        ])?;
    }
    writer.flush()?;
    files.push(reactions_path);

    let edits_path = output_dir.join("edits.csv");
    let mut writer = csv::Writer::from_path(&edits_path)?;
    writer.write_record(["message_guid", "part_index", "status"])?;
    for edit in &bundle.edits {
        writer.write_record([
            edit.message_guid.clone(),
            edit.part_index.to_string(),
            edit.status.clone(),
        ])?;
    }
    writer.flush()?;
    files.push(edits_path);

    let edit_events_path = output_dir.join("edit_events.csv");
    let mut writer = csv::Writer::from_path(&edit_events_path)?;
    writer.write_record([
        "message_guid",
        "part_index",
        "event_index",
        "date",
        "text",
        "guid",
    ])?;
    for edit in &bundle.edits {
        for (event_index, event) in edit.events.iter().enumerate() {
            writer.write_record([
                edit.message_guid.clone(),
                edit.part_index.to_string(),
                event_index.to_string(),
                event.date.clone().unwrap_or_default(),
                event.text.clone(),
                event.guid.clone().unwrap_or_default(),
            ])?;
        }
    }
    writer.flush()?;
    files.push(edit_events_path);

    let components_path = output_dir.join("components.csv");
    let mut writer = csv::Writer::from_path(&components_path)?;
    writer.write_record([
        "message_guid",
        "component_index",
        "kind",
        "run_index",
        "start",
        "end",
        "effects",
        "attachment",
        "emoji_image",
    ])?;
    for message in &bundle.messages {
        for (component_index, component) in message.components.iter().enumerate() {
            if component.runs.is_empty() {
                writer.write_record([
                    message.guid.clone(),
                    component_index.to_string(),
                    component.kind.clone(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                    String::new(),
                ])?;
                continue;
            }
            for (run_index, run) in component.runs.iter().enumerate() {
                writer.write_record([
                    message.guid.clone(),
                    component_index.to_string(),
                    component.kind.clone(),
                    run_index.to_string(),
                    run.start.to_string(),
                    run.end.to_string(),
                    serde_json::to_string(&run.effects).unwrap_or_default(),
                    run.attachment
                        .as_ref()
                        .map(|attachment| serde_json::to_string(attachment).unwrap_or_default())
                        .unwrap_or_default(),
                    run.emoji_image.to_string(),
                ])?;
            }
        }
    }
    writer.flush()?;
    files.push(components_path);

    Ok(())
}

fn write_html(bundle: &ExportBundle, path: &Path) -> Result<()> {
    let mut by_chat: HashMap<i32, Vec<&MessageRecord>> = HashMap::new();
    for message in &bundle.messages {
        if let Some(chat_id) = message.chat_id {
            by_chat.entry(chat_id).or_default().push(message);
        }
    }

    let attachments_by_message: HashMap<&str, Vec<&AttachmentRecord>> = {
        let mut map: HashMap<&str, Vec<&AttachmentRecord>> = HashMap::new();
        for attachment in &bundle.attachments {
            map.entry(attachment.message_guid.as_str())
                .or_default()
                .push(attachment);
        }
        map
    };

    let reactions_by_target: HashMap<&str, Vec<&crate::model::ReactionRecord>> = {
        let mut map: HashMap<&str, Vec<&crate::model::ReactionRecord>> = HashMap::new();
        for reaction in &bundle.reactions {
            if let Some(target) = reaction.target_guid.as_deref() {
                map.entry(target).or_default().push(reaction);
            }
        }
        map
    };

    let edits_by_message: HashMap<&str, Vec<&crate::model::EditRecord>> = {
        let mut map: HashMap<&str, Vec<&crate::model::EditRecord>> = HashMap::new();
        for edit in &bundle.edits {
            map.entry(edit.message_guid.as_str())
                .or_default()
                .push(edit);
        }
        map
    };

    let mut html = String::with_capacity(64 * 1024);
    html.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    html.push_str("<title>iUtil message export</title><style>");
    html.push_str(
        ":root{color-scheme:light dark}body{font-family:system-ui,-apple-system,Segoe UI,sans-serif;max-width:900px;margin:0 auto;padding:24px;line-height:1.45}h1{margin-bottom:4px}.meta{color:#777;font-size:14px;margin-bottom:24px}.chat{border-top:1px solid #8884;margin-top:32px;padding-top:16px}.chat h2{margin-bottom:4px}.participants{color:#777;font-size:13px;margin-bottom:16px}.message{margin:12px 0;padding:10px 14px;border-radius:12px;background:#8881}.message.me{background:#3b82f622}.header{font-size:12px;color:#777;margin-bottom:4px}.text{white-space:pre-wrap;word-break:break-word}.attachments{margin-top:8px}.attachments img{max-width:100%;max-height:320px;border-radius:8px;display:block;margin:4px 0}.attachments a{display:inline-block;margin:2px 8px 2px 0}.reactions{font-size:13px;color:#555;margin-top:6px}.reaction{display:inline-block;background:#8882;border-radius:999px;padding:1px 8px;margin-right:4px}details{margin-top:6px;font-size:13px;color:#666}.badge{font-size:11px;text-transform:uppercase;letter-spacing:.04em;background:#8882;border-radius:4px;padding:1px 5px;margin-left:6px}.missing{color:#b45309}"
    );
    html.push_str("</style></head><body>");
    html.push_str("<h1>iUtil message export</h1>");
    html.push_str(&format!(
        "<div class=\"meta\">{} &middot; iOS {} &middot; {} conversations &middot; {} messages &middot; generated {}</div>",
        escape_html(bundle.source.device_name.as_deref().unwrap_or("iPhone")),
        escape_html(bundle.source.product_version.as_deref().unwrap_or("unknown")),
        bundle.source.chat_count,
        bundle.source.message_count,
        escape_html(&bundle.generated_at),
    ));

    for chat in &bundle.chats {
        render_chat(
            &mut html,
            chat,
            by_chat.get(&chat.id).map(Vec::as_slice).unwrap_or(&[]),
            &attachments_by_message,
            &reactions_by_target,
            &edits_by_message,
        );
    }

    html.push_str("</body></html>");

    let mut file = BufWriter::new(File::create(path).context("Unable to create index.html")?);
    file.write_all(html.as_bytes())?;
    file.flush()?;
    Ok(())
}

fn render_chat(
    html: &mut String,
    chat: &ChatRecord,
    messages: &[&MessageRecord],
    attachments_by_message: &HashMap<&str, Vec<&AttachmentRecord>>,
    reactions_by_target: &HashMap<&str, Vec<&crate::model::ReactionRecord>>,
    edits_by_message: &HashMap<&str, Vec<&crate::model::EditRecord>>,
) {
    html.push_str("<section class=\"chat\">");
    html.push_str(&format!("<h2>{}</h2>", escape_html(&chat.name)));
    html.push_str(&format!(
        "<div class=\"participants\">{} &middot; {} messages</div>",
        escape_html(&chat.participants.join(", ")),
        messages.len()
    ));

    if messages.is_empty() {
        html.push_str("<p class=\"missing\">No messages in this conversation.</p>");
    }

    for message in messages {
        let sender =
            message
                .sender
                .as_deref()
                .unwrap_or(if message.is_from_me { "Me" } else { "Unknown" });
        html.push_str(&format!(
            "<div class=\"message{}\">",
            if message.is_from_me { " me" } else { "" }
        ));
        html.push_str("<div class=\"header\">");
        html.push_str(&escape_html(sender));
        if let Some(date) = &message.date {
            html.push_str(" &middot; ");
            html.push_str(&escape_html(date));
        }
        if let Some(service) = &message.service {
            html.push_str(&format!(
                "<span class=\"badge\">{}</span>",
                escape_html(service)
            ));
        }
        html.push_str("</div>");

        if let Some(text) = &message.text
            && !text.is_empty()
        {
            html.push_str(&format!("<div class=\"text\">{}</div>", escape_html(text)));
        }

        if let Some(attachments) = attachments_by_message.get(message.guid.as_str()) {
            html.push_str("<div class=\"attachments\">");
            for attachment in attachments {
                render_attachment(html, attachment);
            }
            html.push_str("</div>");
        }

        if let Some(reactions) = reactions_by_target.get(message.guid.as_str()) {
            html.push_str("<div class=\"reactions\">");
            for reaction in reactions {
                let label = reaction
                    .reaction
                    .as_deref()
                    .or(reaction.emoji.as_deref())
                    .unwrap_or("reaction");
                html.push_str(&format!(
                    "<span class=\"reaction\">{} {}</span>",
                    escape_html(label),
                    escape_html(reaction.sender.as_deref().unwrap_or(""))
                ));
            }
            html.push_str("</div>");
        }

        if let Some(edits) = edits_by_message.get(message.guid.as_str()) {
            html.push_str("<details><summary>Edit history</summary>");
            for edit in edits {
                html.push_str(&format!(
                    "<div>Part {}: {}</div>",
                    edit.part_index,
                    escape_html(&edit.status)
                ));
                for event in &edit.events {
                    html.push_str(&format!(
                        "<div>&middot; {}{}</div>",
                        event.date.as_deref().map(escape_html).unwrap_or_default(),
                        if event.date.is_some() { ": " } else { "" },
                    ));
                    html.push_str(&format!(
                        "<div style=\"margin-left:1em\">{}</div>",
                        escape_html(&event.text)
                    ));
                }
            }
            html.push_str("</details>");
        }

        html.push_str("</div>");
    }

    html.push_str("</section>");
}

fn render_attachment(html: &mut String, attachment: &AttachmentRecord) {
    let Some(path) = &attachment.export_path else {
        if let Some(error) = &attachment.error {
            html.push_str(&format!(
                "<span class=\"missing\">Attachment unavailable: {}</span>",
                escape_html(error)
            ));
        }
        return;
    };

    let name = attachment
        .transfer_name
        .as_deref()
        .or(attachment.filename.as_deref())
        .unwrap_or(path);
    let is_image = attachment
        .mime_type
        .as_deref()
        .is_some_and(|mime| mime.starts_with("image/"));

    if is_image {
        html.push_str(&format!(
            "<a href=\"{}\"><img src=\"{}\" alt=\"{}\" loading=\"lazy\"></a>",
            escape_html(path),
            escape_html(path),
            escape_html(name)
        ));
    } else {
        html.push_str(&format!(
            "<a href=\"{}\">{}</a>",
            escape_html(path),
            escape_html(name)
        ));
    }
}

fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}
