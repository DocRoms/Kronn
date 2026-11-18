//! What an agent message carries besides its text, stored in the transaction
//! of the message: its actions, its question cards, an audit validation.

use crate::models::DiscussionMessage;
use anyhow::Result;
use rusqlite::Connection;

fn ingest(
    conn: &Connection,
    discussion_id: &str,
    msg: &DiscussionMessage,
    content: &str,
) -> Result<()> {
    super::discussion_actions::ingest_message_actions(conn, discussion_id, &msg.id, content)?;
    let invalid_cards = super::discussion_questions::ingest_message_questions(
        conn,
        discussion_id,
        &msg.id,
        content,
    )?;
    super::discussion_questions::report_invalid_cards(
        conn,
        &msg.id,
        msg.lint_report.as_ref(),
        invalid_cards,
    )?;
    super::audit_validation::on_agent_message(
        conn,
        discussion_id,
        &msg.id,
        content,
        &msg.timestamp.to_rfc3339(),
    )
}

inventory::submit! { crate::core::uplink::AgentMessageIngest(ingest) }
inventory::submit! {
    crate::core::uplink::MessageQuestionIngest(super::discussion_questions::ingest_message_questions)
}
inventory::submit! {
    crate::core::uplink::PageActionIngest(super::live_page_actions::ingest_page_actions)
}
