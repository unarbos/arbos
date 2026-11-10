//! Pending bubble ownership. Room ACKs retain the existing clear-all behavior
//! within that room. This does not infer which messages belong to an ACKed batch.

use std::collections::{HashMap, HashSet};

#[derive(Debug, Default)]
pub(crate) struct PendingMessages {
    rooms: HashMap<String, HashSet<String>>,
}

impl PendingMessages {
    pub(crate) fn insert(&mut self, room: &str, message: String) -> bool {
        self.rooms
            .entry(room.to_owned())
            .or_default()
            .insert(message)
    }

    pub(crate) fn contains(&self, room: &str, message: &str) -> bool {
        self.rooms
            .get(room)
            .is_some_and(|pending| pending.contains(message))
    }

    pub(crate) fn remove(&mut self, room: &str, message: &str) -> bool {
        let Some(pending) = self.rooms.get_mut(room) else {
            return false;
        };
        let removed = pending.remove(message);
        if pending.is_empty() {
            self.rooms.remove(room);
        }
        removed
    }

    pub(crate) fn ack_room(&mut self, room: &str) -> bool {
        self.rooms.remove(room).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::PendingMessages;

    #[test]
    fn one_room_ack_preserves_other_rooms_pending_messages() {
        let mut pending = PendingMessages::default();
        pending.insert("project-chat-a", "message-a".into());
        pending.insert("people-chat-b", "message-b".into());
        assert!(pending.ack_room("project-chat-a"));
        assert!(!pending.contains("project-chat-a", "message-a"));
        assert!(pending.contains("people-chat-b", "message-b"));
    }

    #[test]
    fn project_document_ack_cannot_clear_chat_pending() {
        let mut pending = PendingMessages::default();
        pending.insert("project-chat-a", "message-a".into());
        pending.insert("people-chat-b", "message-b".into());
        assert!(!pending.ack_room("proj:project-a"));
        assert!(pending.contains("project-chat-a", "message-a"));
        assert!(pending.contains("people-chat-b", "message-b"));
    }

    #[test]
    fn lookup_uses_owning_room() {
        let mut pending = PendingMessages::default();
        pending.insert("chat-a", "message-a".into());
        assert!(!pending.contains("chat-b", "message-a"));
        assert!(pending.contains("chat-a", "message-a"));
    }

    #[test]
    fn failed_send_removal_uses_owning_room() {
        let mut pending = PendingMessages::default();
        pending.insert("chat-a", "message-a".into());
        assert!(!pending.remove("chat-b", "message-a"));
        assert!(pending.contains("chat-a", "message-a"));
        assert!(pending.remove("chat-a", "message-a"));
        assert!(!pending.contains("chat-a", "message-a"));
    }

    #[test]
    fn any_ack_still_clears_all_messages_within_its_room() {
        let mut pending = PendingMessages::default();
        assert!(pending.insert("chat-a", "first".into()));
        assert!(!pending.insert("chat-a", "first".into()));
        pending.insert("chat-a", "second".into());
        assert!(pending.ack_room("chat-a"));
        assert!(!pending.contains("chat-a", "first"));
        assert!(!pending.contains("chat-a", "second"));
        assert!(!pending.ack_room("chat-a"));
    }
}
