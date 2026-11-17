use std::path::Path;

use crate::event::{EventPayload, EventStore};
use crate::prompt::PromptCatalog;
use crate::Result;

const PAGE_RECORDS: usize = 256;

#[derive(Debug, Default)]
pub(crate) struct ContextCursor {
    after: u64,
    catalog: Option<PromptCatalog>,
}

impl ContextCursor {
    fn read_assets(&mut self, path: &Path, agent: &str) -> Result<()> {
        loop {
            let page = EventStore::read_page(path, self.after, PAGE_RECORDS)?;
            if page.is_empty() {
                return Ok(());
            }
            for event in page {
                self.after = event.id;
                if let EventPayload::Unknown(value) = event.payload {
                    if value["kind"] == "runtime.context_segment" && value["agent"] == agent {
                        self.catalog = value
                            .get("catalog")
                            .filter(|value| !value.is_null())
                            .map(|value| serde_json::from_value(value.clone()))
                            .transpose()?;
                    }
                }
            }
        }
    }
}

pub(crate) fn read_frozen_catalog(path: &Path, agent: &str) -> Result<Option<PromptCatalog>> {
    let mut cursor = ContextCursor::default();
    cursor.read_assets(path, agent)?;
    Ok(cursor.catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn frozen_assets_are_selected_by_agent_across_bounded_pages() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("events.jsonl");
        let mut writer = EventStore::open(&path).unwrap();
        let mut original = crate::prompt::builtin_prompt_catalog();
        original.system.text = "original main system".into();
        writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.context_segment","agent":"main","catalog":original}),
            ))
            .unwrap();
        for index in 0..300 {
            writer.append(EventPayload::Unknown(json!({"kind":"fixture.other_agent_output","agent":"reviewer","index":index,"output":"x".repeat(4096)}))).unwrap();
        }
        let mut unrelated = original.clone();
        unrelated.system.text = "other agent system".into();
        writer
            .append(EventPayload::Unknown(
                json!({"kind":"runtime.context_segment","agent":"reviewer","catalog":unrelated}),
            ))
            .unwrap();
        assert_eq!(Some(original), read_frozen_catalog(&path, "main").unwrap());
        assert_eq!(None, read_frozen_catalog(&path, "missing").unwrap());
    }
}
