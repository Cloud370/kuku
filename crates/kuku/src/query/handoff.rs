const HANDOFF_OPEN_TAG: &str = "<kuku_handoff>";
const HANDOFF_CLOSE_TAG: &str = "</kuku_handoff>";

#[derive(Debug)]
enum DetectorState {
    UserText,
    TagScan,
    HandoffBody,
    ClosingScan,
    Done,
}

#[derive(Debug)]
pub(super) struct HandoffDetector {
    state: DetectorState,
    user_text: String,
    handoff_text: String,
    tag_buffer: String,
}

impl HandoffDetector {
    pub(super) fn new() -> Self {
        Self {
            state: DetectorState::UserText,
            user_text: String::new(),
            handoff_text: String::new(),
            tag_buffer: String::new(),
        }
    }

    pub(super) fn process(&mut self, chunk: &str) -> Option<String> {
        for ch in chunk.chars() {
            match &self.state {
                DetectorState::UserText => {
                    if ch == '<' {
                        self.tag_buffer.clear();
                        self.tag_buffer.push(ch);
                        self.state = DetectorState::TagScan;
                    } else {
                        self.user_text.push(ch);
                    }
                }
                DetectorState::TagScan => {
                    self.tag_buffer.push(ch);
                    if self.tag_buffer == HANDOFF_OPEN_TAG {
                        self.state = DetectorState::HandoffBody;
                        self.tag_buffer.clear();
                    } else if !HANDOFF_OPEN_TAG.starts_with(&self.tag_buffer) {
                        let buffered = self.tag_buffer.clone();
                        self.tag_buffer.clear();
                        self.state = DetectorState::UserText;
                        for buffered_ch in buffered.chars() {
                            self.user_text.push(buffered_ch);
                        }
                    }
                }
                DetectorState::HandoffBody => {
                    if ch == '<' {
                        self.tag_buffer.clear();
                        self.tag_buffer.push(ch);
                        self.state = DetectorState::ClosingScan;
                    } else {
                        self.handoff_text.push(ch);
                    }
                }
                DetectorState::ClosingScan => {
                    self.tag_buffer.push(ch);
                    if self.tag_buffer == HANDOFF_CLOSE_TAG {
                        self.state = DetectorState::Done;
                        self.tag_buffer.clear();
                    } else if !HANDOFF_CLOSE_TAG.starts_with(&self.tag_buffer) {
                        let buffered = self.tag_buffer.clone();
                        self.tag_buffer.clear();
                        self.state = DetectorState::HandoffBody;
                        for buffered_ch in buffered.chars() {
                            self.handoff_text.push(buffered_ch);
                        }
                    }
                }
                DetectorState::Done => {}
            }
        }

        match &self.state {
            DetectorState::UserText | DetectorState::TagScan => {
                let text = std::mem::take(&mut self.user_text);
                if text.is_empty() {
                    None
                } else {
                    Some(text)
                }
            }
            _ => None,
        }
    }

    pub(super) fn finish(self) -> Option<String> {
        match self.state {
            DetectorState::Done => Some(self.handoff_text),
            DetectorState::HandoffBody | DetectorState::ClosingScan => {
                if !self.handoff_text.is_empty() {
                    Some(self.handoff_text)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handoff_tag_detection_simple() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("Hello "), Some("Hello ".to_string()));
        assert_eq!(state.process("<kuku_handoff>"), None);
        assert_eq!(state.process("## Goal\nDo stuff"), None);
        assert_eq!(state.process("</kuku_handoff>"), None);
        assert_eq!(state.finish(), Some("## Goal\nDo stuff".to_string()));
    }

    #[test]
    fn handoff_tag_split_across_chunks() {
        let mut state = HandoffDetector::new();
        assert_eq!(
            state.process("reply text<kuku_"),
            Some("reply text".to_string())
        );
        assert_eq!(state.process("handoff>summary</kuku_handoff>"), None);
        assert_eq!(state.finish(), Some("summary".to_string()));
    }

    #[test]
    fn no_handoff_tag_returns_none_on_finish() {
        let mut state = HandoffDetector::new();
        assert_eq!(
            state.process("just normal text"),
            Some("just normal text".to_string())
        );
        assert_eq!(state.finish(), None);
    }

    #[test]
    fn handoff_close_tag_split_across_chunks() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("text<kuku_handoff>body"), None);
        assert_eq!(state.process("more</kuku_hand"), None);
        assert_eq!(state.process("off>rest"), None);
        assert_eq!(state.finish(), Some("bodymore".to_string()));
    }

    #[test]
    fn false_start_tag_recovered() {
        let mut state = HandoffDetector::new();
        assert_eq!(state.process("hello <not"), Some("hello <not".to_string()));
        assert_eq!(state.finish(), None);
    }
}
