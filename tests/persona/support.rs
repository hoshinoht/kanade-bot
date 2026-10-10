use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use kanade::chat::persona::{PersonaId, PersonaRoot, ProfileId};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub fn tracked_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("config/personas")
}

pub fn tracked(relative: &str) -> String {
    fs::read_to_string(tracked_dir().join(relative)).unwrap()
}

pub fn pid(value: &str) -> PersonaId {
    PersonaId::parse(value).unwrap()
}

pub fn prof(value: &str) -> ProfileId {
    ProfileId::parse(value).unwrap()
}

/// A synthetic persona directory, removed on drop.
pub struct Fixture {
    base: PathBuf,
    pub dir: PathBuf,
}

impl Fixture {
    pub fn empty() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "kanade-persona-{}-{}-{nanos}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let dir = base.join("personas");
        fs::create_dir_all(&dir).unwrap();
        Self { base, dir }
    }

    /// Bundles/profiles directories plus the tracked Kanade fallback.
    pub fn new() -> Self {
        let fixture = Self::empty();
        fixture.write("bundles/kanade.yaml", tracked("bundles/kanade.yaml"));
        fs::create_dir_all(fixture.dir.join("profiles")).unwrap();
        fixture
    }

    /// A sibling directory outside the persona root.
    pub fn outside(&self) -> PathBuf {
        let outside = self.base.join("outside");
        fs::create_dir_all(&outside).unwrap();
        outside
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    pub fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.path(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    pub fn remove(&self, relative: &str) {
        fs::remove_file(self.path(relative)).unwrap();
    }

    pub fn root(&self) -> PersonaRoot {
        PersonaRoot::open(&self.dir).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

pub fn catalog(default: &str, entries: &[(&str, &[&str])]) -> String {
    let mut out = format!("schema_version: 1\ndefault: {default}\npersonas:\n");
    for (id, aliases) in entries {
        let aliases = aliases
            .iter()
            .map(|alias| format!("'{alias}'"))
            .collect::<Vec<_>>()
            .join(", ");
        out += &format!("  - id: {id}\n    label: Label {id}\n    aliases: [{aliases}]\n");
    }
    out
}

pub fn bundle(id: &str, identity: &str) -> String {
    format!(
        "schema_version: 1
id: {id}
identity: |
  # Persona: {identity}

  Synthetic identity for {identity}.
behaviour:
  voice: Synthetic {id} voice.
  prompt: |
    Synthetic behaviour for {id}.
staging:
  schedule: {id} schedule
  guide: {id} guide
  guide_named: '{{boss}} {id} guide'
  write: {id} write
  generic: {id} generic
"
    )
}

pub fn profile(id: &str, generic: Option<&str>) -> String {
    let staging = generic.map_or_else(String::new, |line| format!("staging:\n  generic: {line}\n"));
    format!(
        "schema_version: 1
id: {id}
label: Profile {id}
voice: Synthetic {id} voice.
prompt: |
  Synthetic profile {id}.
{staging}"
    )
}
