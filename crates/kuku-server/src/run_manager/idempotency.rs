use std::collections::HashMap;

use kuku::event::TaskId;

#[derive(Debug, Default)]
pub struct IdempotencyIndex {
    entries: HashMap<String, (String, TaskId)>,
}

impl IdempotencyIndex {
    pub fn lookup(&self, key: &str, digest: &str) -> Result<Option<TaskId>, ()> {
        match self.entries.get(key) {
            None => Ok(None),
            Some((existing, task_id)) if existing == digest => Ok(Some(task_id.clone())),
            Some(_) => Err(()),
        }
    }

    pub fn insert(&mut self, key: String, digest: String, task_id: TaskId) {
        self.entries.insert(key, (digest, task_id));
    }
}
