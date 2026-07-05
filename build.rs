use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let cards_dir = manifest_dir.join("src/models/game/power_cards");

    println!("cargo:rerun-if-changed={}", cards_dir.display());

    let mut cards = lua_files(&cards_dir);
    cards.sort();

    for card in &cards {
        println!("cargo:rerun-if-changed={}", card.display());
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(
        out_dir.join("power_card_sources.rs"),
        render_power_card_sources(&manifest_dir, &cards),
    )
    .expect("write generated power card sources");
}

fn lua_files(cards_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(cards_dir) else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "lua"))
        .collect()
}

fn render_power_card_sources(manifest_dir: &Path, cards: &[PathBuf]) -> String {
    let mut output = String::from(
        "pub(super) struct PowerCardSource {\n    pub path: &'static str,\n    pub source: &'static str,\n}\n\npub(super) const POWER_CARD_SOURCES: &[PowerCardSource] = &[\n",
    );

    for card in cards {
        let relative = card
            .strip_prefix(manifest_dir)
            .expect("power card path under manifest dir")
            .to_string_lossy()
            .replace('\\', "/");

        output.push_str("    PowerCardSource { path: ");
        output.push_str(&format!("{relative:?}"));
        output.push_str(", source: include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), ");
        output.push_str(&format!("{:?}", format!("/{relative}")));
        output.push_str(")) },\n");
    }

    output.push_str("];\n");
    output
}
