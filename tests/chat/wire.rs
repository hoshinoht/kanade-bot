//! The tracked Kanade persona.

use kanade_voice::persona::{CompiledPersona, PersonaId, PersonaRoot};

/// v4's tracked public Kanade bundle with no reply profile.
pub fn kanade() -> CompiledPersona {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas");
    let root = PersonaRoot::open(&root).expect("tracked personas");
    let id = PersonaId::parse("kanade").expect("persona id");
    CompiledPersona::compile(&root.load_bundle(&id).expect("kanade bundle").value, None)
}
