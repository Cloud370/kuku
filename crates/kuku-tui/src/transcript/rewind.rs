use super::{Body, Key, Transcript, Window};

impl Transcript {
    pub(crate) fn invalidate_after_rewind(&mut self) {
        self.readonly_epoch = self.readonly_epoch.wrapping_add(1);
        self.reset_pending_rewind_group_pages();
        self.reply_position = None;
        self.reading = None;
        self.restore = None;
        self.recent = Window::default();
        self.current_execution = None;
        self.detail_items.visible.clear();
        self.visible_rewind_actions.clear();
        self.focused_rewind_group = None;
    }

    pub(crate) fn rewind_messages(&self, agent: &str) -> Vec<(u64, u64, String)> {
        self.recent
            .records
            .iter()
            .rev()
            .filter_map(|record| {
                if record.owner.as_deref() != Some(agent) {
                    return None;
                }
                let Key::Event(id) = record.key else {
                    return None;
                };
                let Body::User { text, .. } = &record.body else {
                    return None;
                };
                Some((id, record.turn_number.or(record.turn)?, text.clone()))
            })
            .collect()
    }
}
