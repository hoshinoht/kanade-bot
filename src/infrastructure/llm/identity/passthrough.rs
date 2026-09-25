use super::codec::{CodecMode, DecodeError, IdentityCodec, IdentitySession, Member};

/// Pseudonymization off: every method returns exactly what a port would
/// render without a codec, so v5 never depends on the on mode to work.
#[derive(Clone, Copy, Debug, Default)]
pub struct Passthrough;

impl IdentityCodec for Passthrough {
    fn mode(&self) -> CodecMode {
        CodecMode::Passthrough
    }

    fn open(&self, _roster: &[Member]) -> Box<dyn IdentitySession> {
        Box::new(PassthroughSession)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PassthroughSession;

impl IdentitySession for PassthroughSession {
    fn author_label(&mut self, _user_id: &str, name: &str) -> String {
        name.to_owned()
    }

    fn member_ref(&mut self, user_id: &str) -> String {
        user_id.to_owned()
    }

    fn text(&mut self, text: &str) -> String {
        text.to_owned()
    }

    fn tool_result(&mut self, content: &str) -> String {
        content.to_owned()
    }

    fn participant_enum(&self) -> Option<Vec<String>> {
        None
    }

    fn decode_ref(&self, value: &str) -> Result<String, DecodeError> {
        Ok(value.to_owned())
    }

    fn decode_json(&self, json: &str) -> Result<String, DecodeError> {
        Ok(json.to_owned())
    }

    fn decode_reply(&self, text: &str) -> Result<String, DecodeError> {
        Ok(text.to_owned())
    }
}
