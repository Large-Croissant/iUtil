use std::collections::{BTreeSet, HashMap};

use anyhow::{Context, Result};
use imessage_database::{
    chrono::DateTime,
    tables::{
        attachment::Attachment,
        capabilities::Capabilities,
        chat::Chat,
        chat_handle::ChatToHandle,
        handle::Handle,
        messages::{
            Message,
            models::{AttributedRange, BubbleComponent},
        },
        table::get_connection,
    },
    util::{dates::to_utc, query_context::QueryContext},
};

use crate::{
    backup::{BackupSource, attachment_source},
    model::{
        AttachmentMetaRecord, AttachmentRecord, ChatRecord, ComponentRecord, EditEventRecord,
        EditRecord, ExportBundle, MessageRecord, ParticipantRecord, Progress, ReactionRecord,
        RunRecord, SourceInfo,
    },
};

pub fn build_bundle(
    source: &BackupSource,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<ExportBundle> {
    on_progress(Progress {
        fraction: 0.02,
        message: "Opening the Messages database...".to_owned(),
    });

    let db = get_connection(source.database_path())
        .context("Unable to open the Messages database in this backup")?;
    let capabilities =
        Capabilities::determine(&db).context("Unable to inspect the Messages database schema")?;

    on_progress(Progress {
        fraction: 0.08,
        message: "Loading contacts and conversations...".to_owned(),
    });

    let handles = Handle::load(&db).context("Unable to load message participants")?;
    let handle_by_rowid: HashMap<i32, Handle> =
        handles.into_iter().map(|handle| (handle.rowid, handle)).collect();

    let participants_by_chat =
        ChatToHandle::participants_by_chat(&db).context("Unable to load chat participants")?;
    let chats = Chat::by_rowid(&db).context("Unable to load conversations")?;

    let mut participant_chats: HashMap<i32, Vec<i32>> = HashMap::new();
    for (chat_id, handle_ids) in &participants_by_chat {
        for handle_id in handle_ids {
            participant_chats
                .entry(*handle_id)
                .or_default()
                .push(*chat_id);
        }
    }
    for chat_ids in participant_chats.values_mut() {
        chat_ids.sort_unstable();
    }

    let mut participants: Vec<ParticipantRecord> = handle_by_rowid
        .values()
        .map(|handle| ParticipantRecord {
            handle_id: handle.rowid,
            address: handle.id.clone(),
            person_centric_id: handle.person_centric_id.clone(),
            chats: participant_chats
                .get(&handle.rowid)
                .cloned()
                .unwrap_or_default(),
        })
        .collect();
    participants.sort_by_key(|participant| participant.handle_id);

    let mut chat_records: Vec<ChatRecord> = chats
        .values()
        .map(|chat| {
            let participant_ids: BTreeSet<i32> = participants_by_chat
                .get(&chat.rowid)
                .cloned()
                .unwrap_or_default();
            let participants = participant_ids
                .iter()
                .filter_map(|handle_id| {
                    handle_by_rowid
                        .get(handle_id)
                        .map(|handle| handle.id.clone())
                })
                .collect();

            ChatRecord {
                id: chat.rowid,
                identifier: chat.chat_identifier.clone(),
                name: chat.name().to_owned(),
                display_name: chat.display_name().map(str::to_owned),
                service: chat.service_name.clone(),
                participants,
                message_count: 0,
            }
        })
        .collect();
    chat_records.sort_by_key(|chat| chat.id);
    let chat_index: HashMap<i32, usize> = chat_records
        .iter()
        .enumerate()
        .map(|(index, chat)| (chat.id, index))
        .collect();

    on_progress(Progress {
        fraction: 0.15,
        message: "Reading messages...".to_owned(),
    });

    let context = QueryContext::default();
    let mut statement = Message::stream_rows(&db, &capabilities, &context)
        .context("Unable to prepare the message query")?;

    let mut messages = Vec::new();
    let mut attachments = Vec::new();
    let mut reactions = Vec::new();
    let mut edits = Vec::new();

    let mut processed = 0usize;
    for row in Message::rows(&mut statement, [])? {
        let message = row.context("Unable to decode a message row")?;

        if let Some(chat_id) = message.chat_id
            && let Some(index) = chat_index.get(&chat_id)
        {
            chat_records[*index].message_count += 1;
        }

        let sender = message
            .handle_id
            .and_then(|handle_id| handle_by_rowid.get(&handle_id))
            .map(|handle| handle.id.clone());

        let message_attachments =
            Attachment::from_message(&db, &message, &capabilities).unwrap_or_default();
        for attachment in &message_attachments {
            attachments.push(attachment_record(source, &message.guid, attachment));
        }

        if let Some(reaction_type) = message.associated_message_type {
            let target_guid = message
                .clean_associated_guid()
                .map(|(_, guid)| guid.to_owned())
                .or_else(|| message.associated_message_guid.clone());
            reactions.push(ReactionRecord {
                message_guid: message.guid.clone(),
                target_guid,
                reaction_type: Some(reaction_type),
                reaction: reaction_label(reaction_type).map(str::to_owned),
                emoji: message.associated_message_emoji.clone(),
                sender: sender.clone(),
                date: date_string(message.date()),
                is_from_me: message.is_from_me,
            });
        }

        if let Some(edited) = &message.edited_parts {
            for (part_index, part) in edited.parts.iter().enumerate() {
                if part.edit_history.is_empty()
                    && matches!(part.status, imessage_database::message_types::edited::EditStatus::Original)
                {
                    continue;
                }

                edits.push(EditRecord {
                    message_guid: message.guid.clone(),
                    part_index,
                    status: format!("{:?}", part.status),
                    events: part
                        .edit_history
                        .iter()
                        .map(|event| EditEventRecord {
                            date: to_utc(event.date)
                                .ok()
                                .map(|date| date.to_rfc3339()),
                            text: event.text.clone(),
                            guid: event.guid.clone(),
                        })
                        .collect(),
                });
            }
        }

        messages.push(message_record(&message, sender));
        processed += 1;

        if processed.is_multiple_of(500) {
            on_progress(Progress {
                fraction: 0.15,
                message: format!("Read {processed} messages..."),
            });
        }
    }

    messages.sort_by(|left, right| {
        left.date
            .cmp(&right.date)
            .then_with(|| left.rowid.cmp(&right.rowid))
    });

    let attachment_count = attachments.len();
    let message_count = messages.len();

    Ok(ExportBundle {
        generated_at: imessage_database::chrono::Utc::now().to_rfc3339(),
        source: SourceInfo {
            kind: if source.is_encrypted() {
                "ios-backup-encrypted".to_owned()
            } else {
                "ios-backup".to_owned()
            },
            encrypted: source.is_encrypted(),
            device_name: source.device_name().map(str::to_owned),
            product_version: source.product_version().map(str::to_owned),
            chat_count: chat_records.len(),
            message_count,
            attachment_count,
        },
        chats: chat_records,
        participants,
        messages,
        attachments,
        reactions,
        edits,
    })
}

fn message_record(message: &Message, sender: Option<String>) -> MessageRecord {
    MessageRecord {
        rowid: message.rowid,
        guid: message.guid.clone(),
        chat_id: message.chat_id,
        sender,
        is_from_me: message.is_from_me,
        service: message.service.clone(),
        text: message.text.clone(),
        subject: message.subject.clone(),
        date: date_string(message.date()),
        date_delivered: date_string(message.date_delivered()),
        date_read: date_string(message.date_read()),
        date_edited: date_string(message.date_edited()),
        item_type: message.item_type,
        group_title: message.group_title.clone(),
        group_action_type: message.group_action_type,
        associated_message_guid: message.associated_message_guid.clone(),
        associated_message_type: message.associated_message_type,
        associated_message_emoji: message.associated_message_emoji.clone(),
        balloon_bundle_id: message.balloon_bundle_id.clone(),
        expressive_send_style_id: message.expressive_send_style_id.clone(),
        thread_originator_guid: message.thread_originator_guid.clone(),
        thread_originator_part: message.thread_originator_part.clone(),
        num_attachments: message.num_attachments,
        num_replies: message.num_replies,
        deleted_from: message.deleted_from,
        filter_action: message.filter_action,
        filter_sub_action: message.filter_sub_action,
        components: components(&message.components),
    }
}

fn components(components: &[BubbleComponent]) -> Vec<ComponentRecord> {
    components
        .iter()
        .map(|component| match component {
            BubbleComponent::Run(runs) => ComponentRecord {
                kind: "run".to_owned(),
                runs: runs.iter().map(run_record).collect(),
            },
            BubbleComponent::App => ComponentRecord {
                kind: "app".to_owned(),
                runs: Vec::new(),
            },
            BubbleComponent::Retracted => ComponentRecord {
                kind: "retracted".to_owned(),
                runs: Vec::new(),
            },
        })
        .collect()
}

fn run_record(run: &AttributedRange) -> RunRecord {
    RunRecord {
        start: run.start,
        end: run.end,
        effects: run.effects.iter().map(|effect| format!("{effect:?}")).collect(),
        attachment: run.attachment.as_ref().map(|meta| AttachmentMetaRecord {
            guid: meta.guid.clone(),
            transcription: meta.transcription.clone(),
            height: meta.height,
            width: meta.width,
            name: meta.name.clone(),
        }),
        emoji_image: run.emoji_image,
    }
}

fn attachment_record(
    source: &BackupSource,
    message_guid: &str,
    attachment: &Attachment,
) -> AttachmentRecord {
    AttachmentRecord {
        message_guid: message_guid.to_owned(),
        rowid: attachment.rowid,
        guid: attachment.guid.clone(),
        filename: attachment.filename.clone(),
        transfer_name: attachment.transfer_name.clone(),
        uti: attachment.uti.clone(),
        mime_type: attachment.mime_type.clone(),
        total_bytes: attachment.total_bytes,
        is_sticker: attachment.is_sticker,
        hide_attachment: attachment.hide_attachment,
        emoji_description: attachment.emoji_description.clone(),
        export_path: None,
        copied: false,
        error: None,
        source: attachment_source(source, attachment),
    }
}

fn date_string(date: Result<DateTime<imessage_database::chrono::Utc>, impl std::fmt::Debug>) -> Option<String> {
    date.ok().map(|date| date.to_rfc3339())
}

fn reaction_label(reaction_type: i32) -> Option<&'static str> {
    match reaction_type {
        2000 | 3000 => Some("heart"),
        2001 | 3001 => Some("thumbs_up"),
        2002 | 3002 => Some("thumbs_down"),
        2003 | 3003 => Some("haha"),
        2004 | 3004 => Some("emphasis"),
        2005 | 3005 => Some("question"),
        _ => None,
    }
}
