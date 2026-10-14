use super::codec::Member;

/// The sole identity handling mode: model requests carry caller-supplied data unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct Passthrough;

impl Passthrough {
    pub fn open(&self, _roster: &[Member]) -> PassthroughSession {
        PassthroughSession
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PassthroughSession;

impl PassthroughSession {
    pub fn author_label(&mut self, _user_id: &str, name: &str) -> String {
        name.to_owned()
    }

    pub fn member_ref(&mut self, user_id: &str) -> String {
        user_id.to_owned()
    }

    pub fn mention(&mut self, user_id: &str) -> String {
        format!("<@{user_id}>")
    }

    pub fn text(&mut self, text: &str) -> String {
        text.to_owned()
    }

    pub fn tool_result(&mut self, content: &str) -> String {
        content.to_owned()
    }

    pub fn message_ref(&mut self, message_id: &str) -> String {
        message_id.to_owned()
    }
}
