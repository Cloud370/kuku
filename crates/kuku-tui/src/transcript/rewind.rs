use super::{Body, Key, Transcript};

impl Transcript {
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
                Some((id, record.turn?, text.clone()))
            })
            .collect()
    }
}
