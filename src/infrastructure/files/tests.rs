use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{load_catalog, load_knowledge_dir, load_personas};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("kanade-files-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        fs::write(&path, text).expect("write");
        path
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const CATALOG: &str = "difficulties:\n  n: Normal\n  h: Hard\nbosses:\n  Zeta:\n    full: Zeta Boss\n    level: 200\n    aliases: [zeta]\n    guide: {colour: 0x39BFFF}\n  MaleficStar:\n    full: Malefic Star\n    difficulties: [n]\n    aliases: [star]\n";

#[test]
fn tracked_catalog_loads_in_file_order() {
    let table = load_catalog(&repo("boss/bosses.yaml")).expect("tracked catalog");
    let first = table.bosses().first().expect("bosses");
    assert_eq!(first.short(), "Lotus");
    assert!(table.boss("MaleficStar").is_some());
}

#[test]
fn mixed_case_catalog_keeps_order_and_hex_colour() {
    let temp = Temp::new();
    let path = temp.write("Boss/Bosses.yaml", CATALOG);
    let table = load_catalog(&path).expect("catalog");
    let shorts: Vec<_> = table.bosses().iter().map(|boss| boss.short()).collect();
    assert_eq!(shorts, ["Zeta", "MaleficStar"]);
    assert_eq!(
        table.boss("Zeta").and_then(|boss| boss.guide_colour()),
        Some(0x39BFFF)
    );
}

#[test]
fn malformed_catalogs_are_refused_with_the_file_named() {
    let temp = Temp::new();
    for (name, text) in [
        (
            "Unknown.yaml",
            "difficulties: {n: Normal}\nbosses:\n  A:\n    colour: 1\n",
        ),
        (
            "Dup.yaml",
            "difficulties: {n: Normal}\nbosses:\n  A: {}\n  A: {}\n",
        ),
        (
            "Null.yaml",
            "difficulties: {n: Normal}\nbosses:\n  A:\n    full: null\n",
        ),
        (
            "Invalid.yaml",
            "difficulties: {n: Normal}\nbosses:\n  A:\n    difficulties: [q]\n",
        ),
        ("Syntax.yaml", "difficulties: [\n"),
    ] {
        let path = temp.write(name, text);
        let error = load_catalog(&path).expect_err(name);
        assert_eq!(error.file, path.display().to_string(), "{name}");
        assert!(
            !error.to_string().contains("difficulties: {"),
            "{name}: {error}"
        );
    }
}

#[test]
fn tracked_knowledge_validates() {
    let dir = load_knowledge_dir(&repo("boss/knowledge")).expect("tracked knowledge");
    assert!(dir.keys.iter().any(|key| key == "MaleficStar"));
    assert!(dir.keys.iter().any(|key| key == "Kai"));
}

fn knowledge_fixture(temp: &Temp) -> PathBuf {
    let dir = temp.0.join("Knowledge");
    fs::create_dir_all(&dir).expect("dir");
    fs::copy(repo("boss/knowledge/schema.json"), dir.join("schema.json")).expect("schema");
    fs::copy(repo("boss/knowledge/_meta.yaml"), dir.join("_meta.yaml")).expect("meta");
    fs::copy(
        repo("boss/knowledge/maleficstar.yaml"),
        dir.join("maleficstar.yaml"),
    )
    .expect("doc");
    dir
}

#[test]
fn knowledge_fixture_validates_and_refuses_bad_documents() {
    let temp = Temp::new();
    let dir = knowledge_fixture(&temp);
    assert_eq!(
        load_knowledge_dir(&dir).expect("fixture").keys,
        ["MaleficStar"]
    );

    // A mixed-case stem is never found by the API's lowercased lookup.
    fs::rename(dir.join("maleficstar.yaml"), dir.join("MaleficStar.yaml")).expect("rename");
    let error = load_knowledge_dir(&dir).expect_err("mixed-case stem");
    assert!(error.file.ends_with("MaleficStar.yaml"), "{error}");
    fs::rename(dir.join("MaleficStar.yaml"), dir.join("maleficstar.yaml")).expect("rename");

    let bad = dir.join("zeta.yaml");
    fs::write(&bad, "boss: Zeta\nsummary: secret text\n").expect("write");
    let error = load_knowledge_dir(&dir).expect_err("schema violation");
    assert_eq!(error.file, bad.display().to_string());
    assert!(!error.problem.contains("secret text"), "{error}");

    fs::write(&bad, "boss: [\n").expect("write");
    assert_eq!(
        load_knowledge_dir(&dir).expect_err("syntax").file,
        bad.display().to_string()
    );
}

#[test]
fn knowledge_requires_schema_v2_meta() {
    let temp = Temp::new();
    let dir = knowledge_fixture(&temp);
    let meta = dir.join("_meta.yaml");
    fs::write(&meta, "schema_version: 1\n").expect("write");
    assert_eq!(
        load_knowledge_dir(&dir).expect_err("v1").file,
        meta.display().to_string()
    );
}

#[test]
fn personas_fall_back_to_tracked_kanade_without_private_files() {
    let temp = Temp::new();
    let root = temp.0.join("Personas");
    fs::create_dir_all(root.join("bundles")).expect("dir");
    fs::copy(
        repo("config/personas/bundles/kanade.yaml"),
        root.join("bundles/kanade.yaml"),
    )
    .expect("bundle");
    let load = load_personas(&root, None).expect("personas");
    let active = load.snapshot.active().expect("fallback active");
    assert_eq!(active.bundle.value.id.to_string(), "kanade");
    assert!(load.options.is_empty());
}

#[test]
fn personas_list_readable_profiles_but_never_the_example() {
    let temp = Temp::new();
    let root = temp.0.join("Personas");
    fs::create_dir_all(root.join("bundles")).expect("dir");
    fs::create_dir_all(root.join("profiles")).expect("dir");
    fs::copy(
        repo("config/personas/bundles/kanade.yaml"),
        root.join("bundles/kanade.yaml"),
    )
    .expect("bundle");
    fs::copy(
        repo("config/personas/catalog.example.yaml"),
        root.join("catalog.yaml"),
    )
    .expect("catalog");
    let example =
        fs::read_to_string(repo("config/personas/profiles/example.yaml")).expect("example");
    fs::write(root.join("profiles/example.yaml"), &example).expect("example");
    fs::write(
        root.join("profiles/calm.yaml"),
        example.replace("id: example", "id: calm"),
    )
    .expect("calm");
    fs::write(root.join("profiles/broken.yaml"), "id: [\n").expect("broken");
    let load = load_personas(&root, None).expect("personas");
    let keys: Vec<_> = load
        .options
        .iter()
        .map(|option| option.key.as_str())
        .collect();
    assert_eq!(keys, ["calm"]);
}
