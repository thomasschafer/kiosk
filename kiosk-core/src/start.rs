//! Getting a picker ready from a loaded config, the same way in every frontend.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{
    config::{self, LoadedConfig, ResolvedSearchDir, keys::KeysConfig},
    product::Product,
    state::{AppState, ToastKind},
};

/// What [`crate::picker::Picker::start`] needs, with the config's warnings and any removals
/// left pending by an earlier run already in `state`.
pub struct PickerStart {
    pub state: AppState,
    pub search_dirs: Vec<ResolvedSearchDir>,
    pub keys: KeysConfig,
}

/// Prepares the picker for `loaded`, which must list search directories. `current_cwd` is
/// where the user is, for preselecting the repository and branch they are in.
pub fn prepare<E>(
    product: &'static Product,
    loaded: LoadedConfig<E>,
    current_cwd: Option<&Path>,
) -> Result<PickerStart> {
    anyhow::ensure!(
        !loaded.core.search_dirs.is_empty(),
        "the config lists no search directories"
    );
    let home = crate::path::home_dir();
    let resolved = loaded
        .core
        .resolved_search_dirs_with(home.as_deref(), config::probe_search_dir)?;
    let mut warnings = loaded.warnings;
    warnings.extend(resolved.warnings);
    let current_cwd =
        current_cwd.map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path)));
    let mut state = AppState::new(product, current_cwd);
    state.repo_view.missing_optional_dirs = resolved.missing_optional.len();
    warnings.extend(crate::picker::delete::load_pending(&mut state));
    for warning in warnings {
        state.push_toast(ToastKind::Warning, warning.message);
    }
    Ok(PickerStart {
        state,
        search_dirs: resolved.dirs,
        keys: loaded.core.keys,
    })
}
