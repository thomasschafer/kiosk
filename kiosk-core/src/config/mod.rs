use std::{
    collections::HashSet,
    fmt, fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::product::Product;

pub mod keys;
pub mod on_open;
pub mod worktrees;
pub use keys::KeysConfig;
pub use on_open::OnOpenConfig;
pub use worktrees::ManagedWorktreesConfig;

/// The Rust source of the config types, for generating their reference documentation.
pub const SOURCES: &[&str] = &[
    include_str!("mod.rs"),
    include_str!("keys.rs"),
    include_str!("on_open.rs"),
    include_str!("worktrees.rs"),
];
pub const DEFAULT_SEARCH_DEPTH: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
/// A repository search root, written either as a path string or an inline table.
pub enum SearchDirEntry {
    /// String form.
    Simple(
        /// Directory to scan with the default depth of 1. `~` and paths beginning
        /// with `~/` expand from the user's home directory.
        String,
    ),
    /// A path with optional per-directory scan depth, plain-folder override, and
    /// missing-directory tolerance.
    Rich {
        /// Directory to scan. `~` and paths beginning with `~/` expand from the
        /// user's home directory; other relative paths are accepted as written.
        path: String,
        /// Maximum directory depth to scan. The value must be at least 1 and
        /// defaults to 1 when omitted.
        depth: Option<u16>,
        /// Whether to include plain folders for this search root. When omitted,
        /// the global `include_non_git` value is used.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        include_non_git: Option<bool>,
        /// Whether this directory may be absent. A missing optional directory is
        /// skipped without a warning; a path that exists but is not a readable
        /// directory still warns. Defaults to `false`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        optional: bool,
    },
}

impl SearchDirEntry {
    fn path_and_depth(&self) -> (&str, u16) {
        match self {
            Self::Simple(path) => (path, DEFAULT_SEARCH_DEPTH),
            Self::Rich { path, depth, .. } => (path, depth.unwrap_or(DEFAULT_SEARCH_DEPTH)),
        }
    }

    const fn optional(&self) -> bool {
        match self {
            Self::Simple(_) => false,
            Self::Rich { optional, .. } => *optional,
        }
    }

    pub const fn resolved_include_non_git(&self, global: bool) -> bool {
        match self {
            Self::Simple(_) => global,
            Self::Rich {
                include_non_git, ..
            } => match include_non_git {
                Some(value) => *value,
                None => global,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// A color from the terminal's ANSI palette.
pub enum ThemeColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
    DarkGray,
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
/// Every field is optional and falls back to the default shown below.
pub struct ThemeConfig {
    /// Repository-picker accent, used for its active border and selected row.
    pub accent: ThemeColor,
    /// Branch and new-branch accent, used for active borders, selections, and
    /// branch status markers.
    pub secondary: ThemeColor,
    /// Help-view accent, used for its border and selected row.
    pub tertiary: ThemeColor,
    /// Error notification title color.
    pub error: ThemeColor,
    /// Warning notification title and setup-warning color.
    pub warning: ThemeColor,
    /// De-emphasized text color for metadata, counts, inactive content, and
    /// explanatory labels.
    pub muted: ThemeColor,
    /// Inactive list-border color.
    pub border: ThemeColor,
    /// Keyboard-hint color in dialogs, setup, and notifications.
    pub hint: ThemeColor,
    /// Foreground color for text on selected-row accent backgrounds in every view.
    pub highlight_fg: ThemeColor,
    /// Color of the marker shown beside an open repository or worktree.
    pub open: ThemeColor,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: ThemeColor::Magenta,
            secondary: ThemeColor::Cyan,
            tertiary: ThemeColor::Green,
            error: ThemeColor::Red,
            warning: ThemeColor::Yellow,
            muted: ThemeColor::DarkGray,
            border: ThemeColor::DarkGray,
            hint: ThemeColor::Blue,
            highlight_fg: ThemeColor::Black,
            open: ThemeColor::Green,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
/// The settings every kiosk product shares. Products add their own sections through a
/// [`ConfigExtension`].
pub struct CoreConfig {
    /// Directories searched recursively for Git repositories and, when enabled,
    /// plain folders. Entries can be simple strings such as `"~/Code"` or inline
    /// tables such as `{ path = "~/Work", depth = 3 }`, and both forms can be mixed.
    /// Missing directories are reported as warnings unless marked `optional`.
    pub search_dirs: Vec<SearchDirEntry>,
    /// Include plain folders in search results globally. Rich search-directory
    /// entries can override this value with their own `include_non_git` setting.
    #[serde(default)]
    pub include_non_git: bool,
    /// Customize terminal-palette colors used by the picker. Light-terminal users
    /// can set `muted`, `border`, and other colors explicitly.
    pub theme: ThemeConfig,
    /// Customize key bindings grouped by where they are active.
    pub keys: KeysConfig,
}

/// A product's own top-level config sections, read from the same file as
/// [`CoreConfig`]. Its fields must not reuse a core section's name.
pub trait ConfigExtension:
    DeserializeOwned + Serialize + Default + Clone + fmt::Debug + Send + Sync + 'static
{
    /// Checks what the types alone cannot, failing with a message for the user.
    fn validate(&self) -> Result<()>;
}

/// For a product with no sections of its own.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoExtension {}

impl ConfigExtension for NoExtension {
    fn validate(&self) -> Result<()> {
        Ok(())
    }
}

impl CoreConfig {
    pub fn resolved_search_dirs_with(
        &self,
        home: Option<&Path>,
        probe: impl Fn(&Path) -> SearchDirStatus,
    ) -> Result<ResolvedSearchDirs> {
        let expanded = self
            .search_dirs
            .iter()
            .map(|entry| {
                let (path, depth) = entry.path_and_depth();
                expand_tilde(path, home).map(|path| {
                    let dir = ResolvedSearchDir {
                        path,
                        depth,
                        include_non_git: entry.resolved_include_non_git(self.include_non_git),
                    };
                    (dir, entry.optional())
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut resolved = ResolvedSearchDirs {
            dirs: Vec::with_capacity(expanded.len()),
            missing_optional: Vec::new(),
            warnings: Vec::new(),
        };
        for (dir, optional) in expanded {
            let problem = match probe(&dir.path) {
                SearchDirStatus::Directory => {
                    resolved.dirs.push(dir);
                    continue;
                }
                SearchDirStatus::Missing if optional => {
                    resolved.missing_optional.push(dir.path);
                    continue;
                }
                SearchDirStatus::Missing => "does not exist".to_string(),
                SearchDirStatus::NotADirectory => "is not a directory".to_string(),
                SearchDirStatus::Inaccessible(error) => format!("cannot be read ({error})"),
            };
            resolved.warnings.push(ConfigWarning {
                message: format!(
                    "configured search directory {problem}: {}",
                    dir.path.display()
                ),
            });
        }
        Ok(resolved)
    }
}

#[derive(Debug)]
/// What a configured search directory path refers to on disk.
pub enum SearchDirStatus {
    Directory,
    /// Nothing exists at the path, including a symlink whose target is missing.
    Missing,
    /// The path, or one of its ancestors, is not a directory.
    NotADirectory,
    Inaccessible(io::Error),
}

pub fn probe_search_dir(path: &Path) -> SearchDirStatus {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => SearchDirStatus::Directory,
        Ok(_) => SearchDirStatus::NotADirectory,
        Err(error) if error.kind() == io::ErrorKind::NotADirectory => {
            SearchDirStatus::NotADirectory
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => probe_missing_ancestry(path),
        Err(error) => SearchDirStatus::Inaccessible(error),
    }
}

/// Windows reports a path beneath a file as not found, so the nearest existing
/// ancestor decides whether the path is genuinely missing.
fn probe_missing_ancestry(path: &Path) -> SearchDirStatus {
    for ancestor in path.ancestors().skip(1) {
        match fs::metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => return SearchDirStatus::Missing,
            Ok(_) => return SearchDirStatus::NotADirectory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return SearchDirStatus::Inaccessible(error),
        }
    }
    SearchDirStatus::Missing
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigWarning {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSearchDirs {
    pub dirs: Vec<ResolvedSearchDir>,
    /// Optional search directories that were skipped because they do not exist.
    pub missing_optional: Vec<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSearchDir {
    pub path: PathBuf,
    pub depth: u16,
    pub include_non_git: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedConfig<E> {
    pub core: CoreConfig,
    pub extension: E,
    pub path: Option<PathBuf>,
    pub exists: bool,
    pub warnings: Vec<ConfigWarning>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPathResolution {
    pub path: Option<PathBuf>,
    pub warnings: Vec<ConfigWarning>,
}

pub fn resolve_config_path(
    product: &Product,
    get_env: impl Fn(&str) -> Option<String>,
) -> ConfigPathResolution {
    let (path, warnings) = resolve_trusted_file_path(
        product,
        product.config_dir_var,
        ("XDG_CONFIG_HOME", ".config"),
        &get_env,
        "config.toml",
        "config",
    );
    ConfigPathResolution { path, warnings }
}

/// The first trusted location for `file_name`: the directory in `product_dir_var` itself,
/// then the product's directory below the XDG home or its fallback below `HOME`.
pub(crate) fn resolve_trusted_file_path(
    product: &Product,
    product_dir_var: Option<&'static str>,
    (xdg_var, home_fallback): (&'static str, &str),
    get_env: &impl Fn(&str) -> Option<String>,
    file_name: &str,
    directory_kind: &str,
) -> (Option<PathBuf>, Vec<ConfigWarning>) {
    let get = |name: &str| get_env(name).filter(|value| !value.is_empty());
    let candidates = [
        product_dir_var.and_then(|name| get(name).map(|value| (name, PathBuf::from(value), false))),
        get(xdg_var).map(|value| (xdg_var, PathBuf::from(value), true)),
        get("HOME").map(|value| ("HOME", PathBuf::from(value).join(home_fallback), true)),
    ];
    let mut warnings = Vec::new();
    for (source, base, add_product_dir) in candidates.into_iter().flatten() {
        if !base.is_absolute() {
            // A cwd-relative fallback lets a browsed repository supply plugin
            // files, so only absolute environment-derived locations are trusted.
            warnings.push(ConfigWarning {
                message: format!(
                    "refusing relative {directory_kind} directory from {source}: {}",
                    base.display()
                ),
            });
            continue;
        }
        let path = if add_product_dir {
            base.join(product.name).join(file_name)
        } else {
            base.join(file_name)
        };
        return (Some(path), warnings);
    }
    (None, warnings)
}

/// A key that one deserialization pass ignored, and the top-level key it sits under.
struct IgnoredKey {
    path: String,
    top_level: Option<String>,
    is_top_level: bool,
}

/// Deserializes `T` from `contents`, recording the keys it does not know.
fn parse_part<T: DeserializeOwned>(contents: &str) -> Result<(T, Vec<IgnoredKey>)> {
    let mut ignored = Vec::new();
    let deserializer = toml::Deserializer::parse(contents).context("invalid config TOML")?;
    let value = serde_ignored::deserialize(deserializer, |path| {
        ignored.push(IgnoredKey {
            path: path.to_string(),
            top_level: top_level_key(&path),
            is_top_level: matches!(
                &path,
                serde_ignored::Path::Map { parent, .. } if matches!(parent, serde_ignored::Path::Root)
            ),
        });
    })
    .context("invalid config TOML")?;
    Ok((value, ignored))
}

fn top_level_key(path: &serde_ignored::Path<'_>) -> Option<String> {
    use serde_ignored::Path;
    match path {
        Path::Root => None,
        Path::Map {
            parent: Path::Root,
            key,
        } => Some(key.clone()),
        Path::Map { parent, .. }
        | Path::Seq { parent, .. }
        | Path::Some { parent }
        | Path::NewtypeStruct { parent }
        | Path::NewtypeVariant { parent } => top_level_key(parent),
    }
}

/// Parses the core sections and the product's extension from one file.
///
/// Each part is deserialized from the whole file, so its errors keep their positions, and
/// each ignores the other's sections. A key is unknown when it belongs to neither: its
/// top-level section was ignored as a whole by the other part.
pub fn parse_config<E: ConfigExtension>(
    contents: &str,
) -> Result<(CoreConfig, E, Vec<ConfigWarning>)> {
    let (core, core_ignored) = parse_part::<CoreConfig>(contents)?;
    let (extension, extension_ignored) = parse_part::<E>(contents)?;
    validate_core(&core)?;
    extension.validate()?;

    let wholly_ignored = |ignored: &[IgnoredKey]| -> HashSet<String> {
        ignored
            .iter()
            .filter(|key| key.is_top_level)
            .filter_map(|key| key.top_level.clone())
            .collect()
    };
    let core_wholly = wholly_ignored(&core_ignored);
    let extension_wholly = wholly_ignored(&extension_ignored);
    let unknown = |ignored: Vec<IgnoredKey>, other_wholly: &HashSet<String>| -> Vec<IgnoredKey> {
        ignored
            .into_iter()
            .filter(|key| {
                key.top_level
                    .as_ref()
                    .is_some_and(|top_level| other_wholly.contains(top_level))
            })
            .collect()
    };
    let mut seen = HashSet::new();
    let warnings = unknown(core_ignored, &extension_wholly)
        .into_iter()
        .chain(unknown(extension_ignored, &core_wholly))
        .filter(|key| seen.insert(key.path.clone()))
        .map(|key| ConfigWarning {
            message: format!("unknown config key ignored: {}", key.path),
        })
        .collect();
    Ok((core, extension, warnings))
}

pub fn load_config_with<E: ConfigExtension>(
    product: &Product,
    get_env: impl Fn(&str) -> Option<String>,
    read_file: impl Fn(&Path) -> io::Result<Option<String>>,
) -> Result<LoadedConfig<E>> {
    let resolution = resolve_config_path(product, get_env);
    let missing = |path| LoadedConfig {
        core: CoreConfig::default(),
        extension: E::default(),
        path,
        exists: false,
        warnings: resolution.warnings.clone(),
    };
    let Some(path) = resolution.path.clone() else {
        return Ok(missing(None));
    };
    let Some(contents) =
        read_file(&path).with_context(|| format!("failed to read config {}", path.display()))?
    else {
        return Ok(missing(Some(path)));
    };

    let (core, extension, mut parse_warnings) =
        parse_config::<E>(&contents).with_context(|| format!("in {}", path.display()))?;
    let mut warnings = resolution.warnings;
    warnings.append(&mut parse_warnings);
    Ok(LoadedConfig {
        core,
        extension,
        path: Some(path),
        exists: true,
        warnings,
    })
}

pub fn load_config<E: ConfigExtension>(product: &Product) -> Result<LoadedConfig<E>> {
    load_config_with(
        product,
        |name| std::env::var(name).ok(),
        |path| match fs::read_to_string(path) {
            Ok(contents) => Ok(Some(contents)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        },
    )
}

fn validate_core(config: &CoreConfig) -> Result<()> {
    for entry in &config.search_dirs {
        let (path, depth) = entry.path_and_depth();
        if path.trim().is_empty() {
            bail!("search directory path must not be empty");
        }
        if depth == 0 {
            bail!("search directory depth must be at least 1 for {path}");
        }
    }
    Ok(())
}

pub(crate) fn expand_tilde(path: &str, home: Option<&Path>) -> Result<PathBuf> {
    if path == "~" {
        return home
            .map(Path::to_path_buf)
            .ok_or_else(|| anyhow::anyhow!("cannot expand '~' because HOME is unavailable"));
    }
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        return home
            .map(|home| home.join(rest))
            .ok_or_else(|| anyhow::anyhow!("cannot expand '{path}' because HOME is unavailable"));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::product::TEST_PRODUCT;

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct TestExtension {
        extra: ExtraSection,
    }

    #[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct ExtraSection {
        level: u8,
    }

    impl ConfigExtension for TestExtension {
        fn validate(&self) -> Result<()> {
            if self.extra.level > 3 {
                bail!("extra level must be at most 3");
            }
            Ok(())
        }
    }

    fn parse(contents: &str) -> Result<(CoreConfig, Vec<ConfigWarning>)> {
        parse_config::<TestExtension>(contents).map(|(core, _, warnings)| (core, warnings))
    }

    #[test]
    fn extension_sections_are_read_beside_the_core_ones_and_validated() {
        let (core, extension, warnings) =
            parse_config::<TestExtension>("search_dirs = [\"/repos\"]\n[extra]\nlevel = 2\n")
                .unwrap();
        assert_eq!(warnings, vec![]);
        assert_eq!(core.search_dirs, [SearchDirEntry::Simple("/repos".into())]);
        assert_eq!(extension.extra.level, 2);

        let error = parse_config::<TestExtension>("[extra]\nlevel = 9").unwrap_err();
        assert!(format!("{error:#}").contains("at most 3"), "{error:#}");
    }

    #[test]
    fn unknown_keys_are_reported_once_whichever_part_they_sit_under() {
        let (_, _, warnings) = parse_config::<TestExtension>(
            "future = 1\n[theme]\nfuture_theme = 1\n[extra]\nlevel = 1\nfuture_extra = 1\n",
        )
        .unwrap();
        let messages: Vec<_> = warnings.iter().map(|w| w.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "unknown config key ignored: future",
                "unknown config key ignored: theme.future_theme",
                "unknown config key ignored: extra.future_extra",
            ]
        );
    }

    #[test]
    fn a_product_without_an_extension_reports_extension_sections_as_unknown() {
        let (_, _, warnings) = parse_config::<NoExtension>("[extra]\nlevel = 1").unwrap();
        assert_eq!(
            warnings,
            [ConfigWarning {
                message: "unknown config key ignored: extra".into()
            }]
        );
    }

    #[test]
    fn type_errors_keep_their_position_in_the_file() {
        let error =
            parse_config::<TestExtension>("search_dirs = []\n[extra]\nlevel = \"x\"").unwrap_err();
        assert!(format!("{error:#}").contains("line 3"), "{error:#}");
    }

    fn absolute_test_path(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join("kiosk-test-config-tests")
            .join(name)
    }

    fn path_string(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn parses_simple_and_depth_search_directory_forms() {
        let (config, warnings) = parse(
            r#"search_dirs = [
                "~/Development",
                { path = "~/Work", depth = 3 },
                { path = "~/Projects" }
            ]"#,
        )
        .unwrap();
        assert_eq!(warnings, vec![]);
        assert_eq!(
            config.search_dirs,
            [
                SearchDirEntry::Simple("~/Development".into()),
                SearchDirEntry::Rich {
                    path: "~/Work".into(),
                    depth: Some(3),
                    include_non_git: None,
                    optional: false,
                },
                SearchDirEntry::Rich {
                    path: "~/Projects".into(),
                    depth: None,
                    include_non_git: None,
                    optional: false,
                },
            ]
        );
    }

    #[test]
    fn resolves_global_and_per_directory_non_git_settings() {
        let (config, warnings) = parse(
            r#"
include_non_git = true
search_dirs = [
    "~/Inherited",
    { path = "~/AlsoInherited" },
    { path = "~/Included", include_non_git = true },
    { path = "~/Excluded", include_non_git = false },
]
"#,
        )
        .unwrap();

        assert_eq!(warnings, vec![]);
        assert!(config.include_non_git);
        assert!(config.search_dirs[0].resolved_include_non_git(config.include_non_git));
        assert!(config.search_dirs[1].resolved_include_non_git(config.include_non_git));
        assert!(config.search_dirs[2].resolved_include_non_git(config.include_non_git));
        assert!(!config.search_dirs[3].resolved_include_non_git(config.include_non_git));
        assert!(matches!(
            &config.search_dirs[1],
            SearchDirEntry::Rich {
                include_non_git: None,
                ..
            }
        ));
        assert!(matches!(
            &config.search_dirs[2],
            SearchDirEntry::Rich {
                include_non_git: Some(true),
                ..
            }
        ));
        assert!(matches!(
            &config.search_dirs[3],
            SearchDirEntry::Rich {
                include_non_git: Some(false),
                ..
            }
        ));
        assert!(
            config.search_dirs[2].resolved_include_non_git(false),
            "an explicit true must override a false global value"
        );

        let resolved = config
            .resolved_search_dirs_with(Some(Path::new("/home/tester")), |_| {
                SearchDirStatus::Directory
            })
            .unwrap();
        assert_eq!(
            resolved
                .dirs
                .iter()
                .map(|dir| dir.include_non_git)
                .collect::<Vec<_>>(),
            [true, true, true, false]
        );
    }

    #[test]
    fn expands_tilde_with_injected_home_and_preserves_depth() {
        let (config, _) =
            parse(r#"search_dirs = ["~", { path = "~/Work", depth = 4 }, "/absolute"]"#).unwrap();
        let resolved = config
            .resolved_search_dirs_with(Some(Path::new("/home/tester")), |_| {
                SearchDirStatus::Directory
            })
            .unwrap();
        assert_eq!(
            resolved.dirs,
            [
                ResolvedSearchDir {
                    path: PathBuf::from("/home/tester"),
                    depth: 1,
                    include_non_git: false,
                },
                ResolvedSearchDir {
                    path: PathBuf::from("/home/tester/Work"),
                    depth: 4,
                    include_non_git: false,
                },
                ResolvedSearchDir {
                    path: PathBuf::from("/absolute"),
                    depth: 1,
                    include_non_git: false,
                },
            ]
        );
        assert_eq!(resolved.warnings, vec![]);
    }

    #[test]
    fn missing_home_makes_tilde_expansion_fail_loudly() {
        let (config, _) = parse(r#"search_dirs = ["~/Work"]"#).unwrap();
        assert!(
            config
                .resolved_search_dirs_with(None, |_| SearchDirStatus::Directory)
                .is_err()
        );
    }

    #[test]
    fn unusable_search_directories_are_reported_instead_of_silently_dropped() {
        let (config, _) =
            parse(r#"search_dirs = ["/exists", "/missing", "/file", "/denied"]"#).unwrap();
        let resolved = config
            .resolved_search_dirs_with(None, |path| match path.to_str().unwrap() {
                "/exists" => SearchDirStatus::Directory,
                "/missing" => SearchDirStatus::Missing,
                "/file" => SearchDirStatus::NotADirectory,
                "/denied" => SearchDirStatus::Inaccessible(io::ErrorKind::PermissionDenied.into()),
                other => panic!("unexpected probe for {other}"),
            })
            .unwrap();

        assert_eq!(
            resolved.dirs,
            [ResolvedSearchDir {
                path: PathBuf::from("/exists"),
                depth: 1,
                include_non_git: false,
            }]
        );
        assert_eq!(resolved.missing_optional, Vec::<PathBuf>::new());
        let messages = resolved
            .warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 3, "warnings were: {messages:?}");
        assert!(messages[0].contains("does not exist: /missing"));
        assert!(messages[1].contains("is not a directory: /file"));
        assert!(messages[2].contains("cannot be read"));
        assert!(messages[2].contains("/denied"));
    }

    #[test]
    fn optional_search_directories_are_skipped_silently_only_when_missing() {
        let (config, warnings) = parse(
            r#"search_dirs = [
                { path = "/present", optional = true },
                { path = "/missing", optional = true },
                { path = "/file", optional = true },
                { path = "/denied", optional = true },
                { path = "/required-missing", optional = false },
            ]"#,
        )
        .unwrap();
        assert_eq!(warnings, vec![]);
        assert!(config.search_dirs[..4].iter().all(SearchDirEntry::optional));
        assert!(!config.search_dirs[4].optional());

        let resolved = config
            .resolved_search_dirs_with(None, |path| match path.to_str().unwrap() {
                "/present" => SearchDirStatus::Directory,
                "/missing" | "/required-missing" => SearchDirStatus::Missing,
                "/file" => SearchDirStatus::NotADirectory,
                "/denied" => SearchDirStatus::Inaccessible(io::ErrorKind::PermissionDenied.into()),
                other => panic!("unexpected probe for {other}"),
            })
            .unwrap();

        assert_eq!(
            resolved.dirs,
            [ResolvedSearchDir {
                path: PathBuf::from("/present"),
                depth: 1,
                include_non_git: false,
            }]
        );
        assert_eq!(resolved.missing_optional, [PathBuf::from("/missing")]);
        let messages = resolved
            .warnings
            .iter()
            .map(|warning| warning.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(messages.len(), 3, "warnings were: {messages:?}");
        assert!(messages[0].contains("is not a directory: /file"));
        assert!(messages[1].contains("/denied"));
        assert!(messages[2].contains("does not exist: /required-missing"));
    }

    #[test]
    fn optional_defaults_to_false_and_is_omitted_when_serialized() {
        let (config, _) = parse(
            r#"search_dirs = ["/simple", { path = "/rich" }, { path = "/opt", optional = true }]"#,
        )
        .unwrap();
        assert_eq!(
            config
                .search_dirs
                .iter()
                .map(SearchDirEntry::optional)
                .collect::<Vec<_>>(),
            [false, false, true]
        );

        let serialized = toml::to_string(&config.search_dirs[1]).unwrap();
        assert!(!serialized.contains("optional"), "serialized: {serialized}");
        let serialized = toml::to_string(&config.search_dirs[2]).unwrap();
        assert!(
            serialized.contains("optional = true"),
            "serialized: {serialized}"
        );
    }

    #[test]
    fn probe_distinguishes_missing_paths_from_other_problems() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "").unwrap();

        assert!(matches!(
            probe_search_dir(temp.path()),
            SearchDirStatus::Directory
        ));
        assert!(matches!(
            probe_search_dir(&temp.path().join("missing")),
            SearchDirStatus::Missing
        ));
        assert!(matches!(
            probe_search_dir(&file),
            SearchDirStatus::NotADirectory
        ));
        assert!(
            matches!(
                probe_search_dir(&file.join("child")),
                SearchDirStatus::NotADirectory
            ),
            "a path beneath a file is misconfigured, not missing"
        );
        assert!(
            matches!(
                probe_search_dir(&file.join("child").join("grandchild")),
                SearchDirStatus::NotADirectory
            ),
            "a path several levels beneath a file is misconfigured, not missing"
        );
        assert!(matches!(
            probe_search_dir(&temp.path().join("missing").join("child")),
            SearchDirStatus::Missing
        ));
    }

    #[cfg(unix)]
    #[test]
    fn probe_treats_a_dangling_symlink_as_missing() {
        let temp = tempfile::tempdir().unwrap();
        let link = temp.path().join("unmounted");
        std::os::unix::fs::symlink(temp.path().join("absent-target"), &link).unwrap();

        assert!(matches!(probe_search_dir(&link), SearchDirStatus::Missing));
    }

    #[test]
    fn config_path_uses_documented_precedence() {
        let plugin = absolute_test_path("plugin-config");
        let xdg = absolute_test_path("xdg");
        let home = absolute_test_path("home");
        let values = HashMap::from([
            ("KIOSK_TEST_CONFIG_DIR", path_string(&plugin)),
            ("XDG_CONFIG_HOME", path_string(&xdg)),
            ("HOME", path_string(&home)),
        ]);
        let resolution = resolve_config_path(&TEST_PRODUCT, |name| values.get(name).cloned());
        assert_eq!(resolution.path, Some(plugin.join("config.toml")));
    }

    #[test]
    fn relative_config_fallbacks_are_refused() {
        let values = HashMap::from([
            ("KIOSK_TEST_CONFIG_DIR", "plugin-config"),
            ("XDG_CONFIG_HOME", ".config"),
            ("HOME", "relative-home"),
        ]);
        let resolution = resolve_config_path(&TEST_PRODUCT, |name| {
            values.get(name).map(ToString::to_string)
        });
        assert!(resolution.path.is_none());
        assert_eq!(resolution.warnings.len(), 3);
        assert!(
            resolution
                .warnings
                .iter()
                .all(|warning| warning.message.contains("refusing relative"))
        );
    }

    #[test]
    fn relative_higher_priority_value_falls_through_to_absolute_xdg() {
        let xdg = absolute_test_path("xdg-fallback");
        let values = HashMap::from([
            ("KIOSK_TEST_CONFIG_DIR", "plugin-config".to_string()),
            ("XDG_CONFIG_HOME", path_string(&xdg)),
        ]);
        let resolution = resolve_config_path(&TEST_PRODUCT, |name| values.get(name).cloned());
        assert_eq!(resolution.path, Some(xdg.join("kiosk-test/config.toml")));
        assert_eq!(resolution.warnings.len(), 1);
    }

    #[test]
    fn unknown_keys_are_ignored_with_warnings() {
        let (config, warnings) = parse(
            r#"
search_dirs = []
future_root_key = true

[theme]
future_theme_key = "blue"
"#,
        )
        .unwrap();
        assert_eq!(config.search_dirs, vec![]);
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("future_root_key"))
        );
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains("future_theme_key"))
        );
    }

    #[test]
    fn theme_accepts_named_terminal_colors_and_rejects_truecolor_values() {
        let (config, warnings) = parse(
            r#"
[theme]
accent = "cyan"
secondary = "blue"
tertiary = "yellow"
highlight_fg = "reset"
"#,
        )
        .unwrap();
        assert_eq!(warnings, vec![]);
        assert_eq!(config.theme.accent, ThemeColor::Cyan);
        assert_eq!(config.theme.secondary, ThemeColor::Blue);
        assert_eq!(config.theme.tertiary, ThemeColor::Yellow);
        assert_eq!(config.theme.highlight_fg, ThemeColor::Reset);
        assert!(parse("[theme]\naccent = \"#ff00ff\"").is_err());
        assert!(parse("[theme]\nsecondary = \"#00ffff\"").is_err());
        assert!(parse("[theme]\ntertiary = [0, 255, 0]").is_err());
        assert!(parse("[theme]\naccent = [255, 0, 255]").is_err());
    }

    #[test]
    fn missing_config_loads_empty_defaults_without_reading_relative_paths() {
        let loaded = load_config_with::<TestExtension>(
            &TEST_PRODUCT,
            |_| None,
            |_| panic!("no path should be read"),
        )
        .unwrap();
        assert_eq!(loaded.core.search_dirs, vec![]);
        assert!(loaded.path.is_none());
        assert!(!loaded.exists);
    }

    #[test]
    fn injected_reader_loads_config() {
        let home = absolute_test_path("injected-home");
        let expected = home.join(".config/kiosk-test/config.toml");
        let loaded = load_config_with::<TestExtension>(
            &TEST_PRODUCT,
            |name| (name == "HOME").then(|| path_string(&home)),
            |path| {
                assert_eq!(path, expected);
                Ok(Some("search_dirs = [\"/repos\"]".into()))
            },
        )
        .unwrap();
        assert_eq!(loaded.core.search_dirs.len(), 1);
        assert!(loaded.exists);
    }

    #[test]
    fn resolved_but_missing_and_existing_empty_configs_are_distinguished() {
        let config_dir = absolute_test_path("resolved-config");
        let missing = load_config_with::<TestExtension>(
            &TEST_PRODUCT,
            |name| (name == "KIOSK_TEST_CONFIG_DIR").then(|| path_string(&config_dir)),
            |_| Ok(None),
        )
        .unwrap();
        assert_eq!(
            missing.path.as_deref(),
            Some(config_dir.join("config.toml").as_path())
        );
        assert!(!missing.exists);

        let empty = load_config_with::<TestExtension>(
            &TEST_PRODUCT,
            |name| (name == "KIOSK_TEST_CONFIG_DIR").then(|| path_string(&config_dir)),
            |_| Ok(Some("search_dirs = []".into())),
        )
        .unwrap();
        assert!(empty.exists);
        assert_eq!(empty.core.search_dirs, vec![]);
    }

    #[test]
    fn invalid_key_chords_and_unknown_actions_are_config_errors() {
        let chord = parse("[keys.branch_select]\n\"Hyper-B\" = \"new_branch\"").unwrap_err();
        assert!(format!("{chord:#}").contains("invalid key chord"));
        let action = parse("[keys.branch_select]\n\"C-b\" = \"unknown_action\"").unwrap_err();
        assert!(format!("{action:#}").contains("unknown key action"));
    }

    #[test]
    fn invalid_config_is_an_error_and_zero_depth_is_rejected() {
        assert!(parse("search_dirs = 1").is_err());
        assert!(parse(r#"search_dirs = [{ path = "/repos", depth = 0 }]"#).is_err());
    }
}
