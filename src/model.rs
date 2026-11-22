use std::path::PathBuf;

use serde::Serialize;

/// Everything produced by one export run.
#[derive(Debug, Serialize)]
pub struct ExportBundle {
    pub generated_at: String,
    pub source: SourceInfo,
    pub chats: Vec<ChatRecord>,
    pub participants: Vec<ParticipantRecord>,
    pub messages: Vec<MessageRecord>,
    pub attachments: Vec<AttachmentRecord>,
    pub reactions: Vec<ReactionRecord>,
    pub edits: Vec<EditRecord>,
}

#[derive(Debug, Serialize)]
pub struct SourceInfo {
    pub kind: String,
    pub encrypted: bool,
    pub device_name: Option<String>,
    pub product_version: Option<String>,
    pub chat_count: usize,
    pub message_count: usize,
    pub attachment_count: usize,
}

#[derive(Debug, Serialize)]
pub struct ChatRecord {
    pub id: i32,
    pub identifier: String,
    pub name: String,
    pub display_name: Option<String>,
    pub service: Option<String>,
    pub participants: Vec<String>,
    pub message_count: usize,
}

#[derive(Debug, Serialize)]
pub struct ParticipantRecord {
    pub handle_id: i32,
    pub address: String,
    pub person_centric_id: Option<String>,
    pub chats: Vec<i32>,
}

#[derive(Debug, Serialize)]
pub struct MessageRecord {
    pub rowid: i32,
    pub guid: String,
    pub chat_id: Option<i32>,
    pub sender: Option<String>,
    pub is_from_me: bool,
    pub service: Option<String>,
    pub text: Option<String>,
    pub subject: Option<String>,
    pub date: Option<String>,
    pub date_delivered: Option<String>,
    pub date_read: Option<String>,
    pub date_edited: Option<String>,
    pub item_type: i32,
    pub group_title: Option<String>,
    pub group_action_type: i32,
    pub associated_message_guid: Option<String>,
    pub associated_message_type: Option<i32>,
    pub associated_message_emoji: Option<String>,
    pub balloon_bundle_id: Option<String>,
    pub expressive_send_style_id: Option<String>,
    pub thread_originator_guid: Option<String>,
    pub thread_originator_part: Option<String>,
    pub num_attachments: i32,
    pub num_replies: i32,
    pub deleted_from: Option<i32>,
    pub filter_action: Option<i32>,
    pub filter_sub_action: Option<i32>,
    pub components: Vec<ComponentRecord>,
}

#[derive(Debug, Serialize)]
pub struct ComponentRecord {
    pub kind: String,
    pub runs: Vec<RunRecord>,
}

#[derive(Debug, Serialize)]
pub struct RunRecord {
    pub start: usize,
    pub end: usize,
    pub effects: Vec<String>,
    pub attachment: Option<AttachmentMetaRecord>,
    pub emoji_image: bool,
}

#[derive(Debug, Serialize)]
pub struct AttachmentMetaRecord {
    pub guid: Option<String>,
    pub transcription: Option<String>,
    pub height: Option<f64>,
    pub width: Option<f64>,
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AttachmentRecord {
    pub message_guid: String,
    pub rowid: i32,
    pub guid: Option<String>,
    pub filename: Option<String>,
    pub transfer_name: Option<String>,
    pub uti: Option<String>,
    pub mime_type: Option<String>,
    pub total_bytes: i64,
    pub is_sticker: bool,
    pub hide_attachment: i32,
    pub emoji_description: Option<String>,
    pub export_path: Option<String>,
    pub copied: bool,
    pub error: Option<String>,
    #[serde(skip)]
    pub source: Option<AttachmentSource>,
}

#[derive(Debug, Clone)]
pub enum AttachmentSource {
    File(PathBuf),
    Encrypted(String),
}

#[derive(Debug, Serialize)]
pub struct ReactionRecord {
    pub message_guid: String,
    pub target_guid: Option<String>,
    pub reaction_type: Option<i32>,
    pub reaction: Option<String>,
    pub emoji: Option<String>,
    pub sender: Option<String>,
    pub date: Option<String>,
    pub is_from_me: bool,
}

#[derive(Debug, Serialize)]
pub struct EditRecord {
    pub message_guid: String,
    pub part_index: usize,
    pub status: String,
    pub events: Vec<EditEventRecord>,
}

#[derive(Debug, Serialize)]
pub struct EditEventRecord {
    pub date: Option<String>,
    pub text: String,
    pub guid: Option<String>,
}
