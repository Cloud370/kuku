#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    prompt: String,
    session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutput {
    pub session_id: String,
    pub text: String,
}

pub fn query(prompt: impl Into<String>) -> Query {
    Query::new(prompt)
}

impl Query {
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            session_id: None,
        }
    }

    pub fn session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }
}
