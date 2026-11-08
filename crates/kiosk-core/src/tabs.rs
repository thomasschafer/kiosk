//! Kiosk's tabs: the picker and the views beside it. Which tabs exist is fixed, so each is a
//! [`TabId`] variant; which one shows is [`Tabs`]' to decide.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

/// Declares [`TabId`] with each tab's name, as config and the wire spell it, and its title.
macro_rules! tabs {
    ($($variant:ident => $name:literal, $title:literal;)+) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum TabId {
            $($variant,)+
        }

        impl TabId {
            /// Every tab, in the order the tab bar shows them.
            pub const ALL: &[Self] = &[$(Self::$variant,)+];

            /// The tab's name, as config, the command line and the wire spell it.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name,)+
                }
            }

            /// What the tab bar calls the tab.
            pub const fn title(self) -> &'static str {
                match self {
                    $(Self::$variant => $title,)+
                }
            }

            /// The `[keys]` action that shows the tab.
            pub const fn command_name(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!($name, "_tab"),)+
                }
            }

            pub const fn command_description(self) -> &'static str {
                match self {
                    $(Self::$variant => concat!("Show the ", $title, " tab"),)+
                }
            }
        }
    };
}

tabs! {
    Picker => "picker", "Repositories";
    Diff => "diff", "Diff";
}

impl TabId {
    /// The tab whose `[keys]` action is `name`.
    pub fn from_command_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|tab| tab.command_name() == name)
    }
}

impl fmt::Display for TabId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl FromStr for TabId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|tab| tab.name() == value)
            .ok_or_else(|| {
                let names = Self::ALL
                    .iter()
                    .map(|tab| format!("'{tab}'"))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("unknown tab '{value}'; the tabs are {names}")
            })
    }
}

/// A change of tab the person asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabCommand {
    Next,
    Previous,
    Show(TabId),
}

/// What a request to launch a tab, from a key bound in the app, comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launched {
    /// The tab is showing now.
    Shown,
    /// The tab was already showing, so the launch closes kiosk, as pressing the key that
    /// opened it does.
    Close,
}

/// The tabs kiosk shows, and which of them shows now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tabs {
    /// The tabs in the tab bar, in its order; never empty.
    available: &'static [TabId],
    /// One of `available`.
    active: TabId,
}

impl Tabs {
    /// Every tab, or with `experimental` off only the picker, opening on `active` if it is
    /// one of them and on the first otherwise.
    pub fn new(experimental: bool, active: TabId) -> Self {
        let available = if experimental {
            TabId::ALL
        } else {
            &[TabId::Picker]
        };
        let mut tabs = Self {
            available,
            active: available[0],
        };
        tabs.active = tabs.or_first(active);
        tabs
    }

    pub const fn active(self) -> TabId {
        self.active
    }

    pub const fn available(self) -> &'static [TabId] {
        self.available
    }

    /// `tab`, if kiosk shows it, or else the first tab.
    fn or_first(self, tab: TabId) -> TabId {
        if self.available.contains(&tab) {
            tab
        } else {
            self.available[0]
        }
    }

    /// The tab `offset` places along the tab bar from the one showing, wrapping around.
    fn step(self, offset: isize) -> TabId {
        let position = self
            .available
            .iter()
            .position(|tab| *tab == self.active)
            .expect("the active tab is available");
        let count = self.available.len().cast_signed();
        self.available[(position.cast_signed() + offset)
            .rem_euclid(count)
            .cast_unsigned()]
    }

    pub fn apply(&mut self, command: TabCommand) {
        self.active = match command {
            TabCommand::Next => self.step(1),
            TabCommand::Previous => self.step(-1),
            TabCommand::Show(tab) => self.or_first(tab),
        };
    }

    /// A tab kiosk doesn't show launches the first, so its key still toggles kiosk.
    pub fn launch(&mut self, tab: TabId) -> Launched {
        let tab = self.or_first(tab);
        if self.active == tab {
            return Launched::Close;
        }
        self.active = tab;
        Launched::Shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_unknown_names_list_the_tabs() {
        for tab in TabId::ALL {
            assert_eq!(tab.name().parse::<TabId>(), Ok(*tab));
        }
        let error = "logs".parse::<TabId>().unwrap_err();
        assert!(error.contains("'picker', 'diff'"), "{error}");
    }

    #[test]
    fn names_match_the_wire_spelling() {
        for tab in TabId::ALL {
            assert_eq!(
                serde_json::to_string(tab).unwrap(),
                format!("\"{}\"", tab.name())
            );
        }
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut tabs = Tabs::new(true, TabId::Picker);
        tabs.apply(TabCommand::Previous);
        assert_eq!(tabs.active(), TabId::ALL[TabId::ALL.len() - 1]);
        tabs.apply(TabCommand::Next);
        assert_eq!(tabs.active(), TabId::ALL[0]);
        for expected in TabId::ALL.iter().skip(1).chain(&TabId::ALL[..1]) {
            tabs.apply(TabCommand::Next);
            assert_eq!(tabs.active(), *expected);
        }
    }

    #[test]
    fn launching_the_tab_already_showing_closes() {
        let mut tabs = Tabs::new(true, TabId::Picker);
        assert_eq!(tabs.launch(TabId::Diff), Launched::Shown);
        assert_eq!(tabs.active(), TabId::Diff);
        assert_eq!(tabs.launch(TabId::Diff), Launched::Close);
        assert_eq!(tabs.active(), TabId::Diff);
    }

    #[test]
    fn without_experimental_tabs_only_the_picker_shows() {
        let mut tabs = Tabs::new(false, TabId::Diff);
        assert_eq!(tabs.available(), [TabId::Picker]);
        assert_eq!(tabs.active(), TabId::Picker);
        for command in [
            TabCommand::Next,
            TabCommand::Previous,
            TabCommand::Show(TabId::Diff),
        ] {
            tabs.apply(command);
            assert_eq!(tabs.active(), TabId::Picker, "{command:?}");
        }
        assert_eq!(tabs.launch(TabId::Diff), Launched::Close);
    }
}
