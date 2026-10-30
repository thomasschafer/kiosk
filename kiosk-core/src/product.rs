//! What distinguishes one kiosk product from another outside its backend: its name and
//! where it keeps its files.

/// A kiosk product, such as the herdr plugin or the tmux picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Product {
    /// The product's name. It names the product's directories below the XDG config and
    /// state homes, titles its notifications and prefixes messages it prints.
    pub name: &'static str,
    /// An environment variable naming a directory that holds `config.toml` directly, tried
    /// before the XDG locations. Plugin hosts set one for their plugins.
    pub config_dir_var: Option<&'static str>,
    /// Like `config_dir_var`, for the state the product keeps between runs.
    pub state_dir_var: Option<&'static str>,
    /// How the product names the multiplexer it drives in prose, as in "It will also be
    /// closed in Herdr.".
    pub backend_name: &'static str,
    /// Shown when the user asks to open something but no backend is available.
    pub no_backend_message: &'static str,
    /// The setup wizard's one-sentence description of what the product does.
    pub setup_intro: &'static str,
}

#[cfg(any(test, feature = "test-support"))]
pub const TEST_PRODUCT: Product = Product {
    name: "kiosk-test",
    config_dir_var: Some("KIOSK_TEST_CONFIG_DIR"),
    state_dir_var: Some("KIOSK_TEST_STATE_DIR"),
    backend_name: "the test backend",
    no_backend_message: "no test backend",
    setup_intro: "Pick repositories and branches in tests.",
};
