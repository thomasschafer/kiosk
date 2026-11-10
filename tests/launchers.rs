//! The launcher mods (`mods/kiosk-<tab>`) that open kiosk on a tab from a key bound in the
//! app. Mods can't share code, so each is a copy of one file; these checks keep the copies,
//! and the shared state they agree on with kiosk's own mod, from drifting apart.

use std::{collections::BTreeSet, fs, path::Path};

use kiosk_core::tabs::TabId;

const MODS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/mods");

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The value of `const <name> = "...";` in `source`.
fn constant<'a>(source: &'a str, name: &str) -> Option<&'a str> {
    let prefix = format!("const {name} = \"");
    source.lines().find_map(|line| {
        line.strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix("\";"))
    })
}

/// Every tab but the first, which `mod.open:kiosk` opens on.
fn launched_tabs() -> &'static [TabId] {
    &TabId::ALL[1..]
}

#[test]
fn every_tab_but_the_first_has_a_launcher_and_no_launcher_names_another() {
    let expected = launched_tabs()
        .iter()
        .map(|tab| format!("kiosk-{tab}"))
        .collect::<BTreeSet<_>>();
    let found = fs::read_dir(MODS)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.starts_with("kiosk-"))
        .collect::<BTreeSet<_>>();
    assert_eq!(found, expected);
}

#[test]
fn launchers_are_one_file_each_with_its_own_tab() {
    let mut bodies = BTreeSet::new();
    for tab in launched_tabs() {
        let dir = Path::new(MODS).join(format!("kiosk-{tab}"));
        let source = read(&dir.join("main.js"));
        assert_eq!(
            constant(&source, "TAB"),
            Some(tab.name()),
            "{}",
            dir.display()
        );
        bodies.insert(source.replace(&format!("const TAB = \"{tab}\";"), ""));
        let manifest: serde_json::Value =
            serde_json::from_str(&read(&dir.join("mod.json"))).unwrap();
        assert_eq!(manifest["place"], "none", "{}", dir.display());
        assert_eq!(manifest["overlay"]["frame"], "none", "{}", dir.display());
    }
    assert!(bodies.len() <= 1, "the launchers differ beyond their TAB");
}

#[test]
fn launchers_and_kiosk_agree_on_their_shared_state() {
    let kiosk = read(&Path::new(MODS).join("kiosk/main.js"));
    for tab in launched_tabs() {
        let launcher = read(&Path::new(MODS).join(format!("kiosk-{tab}/main.js")));
        for name in ["LAUNCH", "OPEN", "ACK"] {
            let ours = constant(&launcher, name);
            assert!(ours.is_some(), "kiosk-{tab} has no {name}");
            assert_eq!(
                ours,
                constant(&kiosk, name),
                "{name} differs in kiosk-{tab}"
            );
        }
    }
}
