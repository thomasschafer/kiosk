//! What the tab bar and the tabs beside the picker show.

use serde::Serialize;

use crate::{
    config::keys::{Command, KeyLayer, KeysConfig},
    tabs::{TabId, Tabs},
};

use super::{PickerView, diff::DiffView};

/// Kiosk's tabs and the one that shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabsView {
    /// Empty while kiosk shows only one tab.
    pub bar: Vec<TabEntry>,
    pub body: TabBody,
}

/// A tab in the tab bar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TabEntry {
    pub id: TabId,
    pub title: &'static str,
    /// The key that shows the tab, if one is bound.
    pub key: Option<String>,
    pub active: bool,
}

/// What the tab that shows draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "tab", rename_all = "snake_case")]
#[expect(
    clippy::large_enum_variant,
    reason = "one body is built per view and sent as it is"
)]
pub enum TabBody {
    Picker(PickerView),
    Diff(DiffView),
}

impl TabBody {
    pub const fn tab(&self) -> TabId {
        match self {
            Self::Picker(_) => TabId::Picker,
            Self::Diff(_) => TabId::Diff,
        }
    }
}

/// The tab bar of `tabs` over `body`, the tab that shows.
pub fn tabs(tabs: Tabs, body: TabBody, keys: &KeysConfig) -> TabsView {
    assert_eq!(body.tab(), tabs.active(), "the body is the active tab's");
    let active = tabs.active();
    let shown = if tabs.available().len() > 1 {
        tabs.available()
    } else {
        &[]
    };
    let bar = shown
        .iter()
        .map(|&id| TabEntry {
            id,
            title: id.title(),
            key: keys
                .layer_key(KeyLayer::Tabs, Command::ShowTab(id))
                .map(|key| key.to_string()),
            active: id == active,
        })
        .collect();
    TabsView { bar, body }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff::tab::DiffTab,
        view::{diff::diff, tests::empty_picker_view},
    };

    #[test]
    fn the_bar_lists_every_tab_with_its_key_and_marks_the_one_showing() {
        let keys = KeysConfig::default();
        let view = tabs(
            Tabs::new(true, TabId::Diff),
            TabBody::Diff(diff(&mut DiffTab::new(None), &keys, 10)),
            &keys,
        );
        let bar = view
            .bar
            .iter()
            .map(|entry| (entry.id, entry.key.as_deref(), entry.active))
            .collect::<Vec<_>>();
        assert_eq!(
            bar,
            [
                (TabId::Picker, Some("ctrl+1"), false),
                (TabId::Diff, Some("ctrl+2"), true),
            ]
        );
        let picker = tabs(
            Tabs::new(true, TabId::Picker),
            TabBody::Picker(empty_picker_view()),
            &keys,
        );
        assert!(picker.bar[0].active && !picker.bar[1].active);
    }

    #[test]
    fn a_lone_tab_has_no_bar() {
        let keys = KeysConfig::default();
        let view = tabs(
            Tabs::new(false, TabId::Picker),
            TabBody::Picker(empty_picker_view()),
            &keys,
        );
        assert_eq!(view.bar, []);
    }

    #[test]
    fn an_unbound_tab_has_no_key_in_the_bar() {
        let keys = KeysConfig::from_toml("[tabs]\n\"ctrl+2\" = \"noop\"\n").unwrap();
        let view = tabs(
            Tabs::new(true, TabId::Diff),
            TabBody::Diff(diff(&mut DiffTab::new(None), &keys, 10)),
            &keys,
        );
        assert_eq!(view.bar[1].key, None);
    }
}
