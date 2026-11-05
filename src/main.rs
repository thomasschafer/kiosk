use std::sync::Arc;

use anyhow::{Context, Result};
use kiosk_core::{
    backend::WorktreePlacement,
    config::{self, ConfigWarning},
    git::{CliGitProvider, GitProvider},
};

use herdr_app_kiosk::{
    PRODUCT,
    app::{self, CliAppHost, HerdrApp, WORKTREE_ROOT},
    background::Background,
    config::Config,
    launch::{self, Invocation, ServeArgs},
    migrate,
    notices::{LaterNotices, LogNotices, NoticeFile},
};

fn main() {
    if let Err(error) = start() {
        eprintln!("{}: {error:#}", PRODUCT.name);
        std::process::exit(1);
    }
}

fn start() -> Result<()> {
    match launch::invocation()? {
        Invocation::Version => {
            println!("{} {}", PRODUCT.name, env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Invocation::Serve(args) => {
            let background = Arc::new(Background::threads());
            let served = kiosk_serve::serve(&PRODUCT, |notifier| {
                prepare(args, notifier, Arc::clone(&background))
            });
            // The popup has closed by now, but what was opened may still be getting its
            // `[on_open]` panes.
            background.wait();
            served
        }
    }
}

fn prepare(
    args: ServeArgs,
    notifier: kiosk_serve::Notifier,
    background: Arc<Background>,
) -> Result<kiosk_serve::Start<HerdrApp, Config>> {
    let carried_over = migrate::carry_over_earlier_config(|name| std::env::var(name).ok())
        .context("copying the earlier app mod's config")?;
    let mut loaded = config::load_config::<Config>(&PRODUCT)?;
    loaded.warnings.extend(
        carried_over
            .into_iter()
            .chain(app::ignored_ratio_warning(&loaded.extension.on_open))
            .map(|message| ConfigWarning { message }),
    );
    let home = kiosk_core::path::home_dir().context("HOME must be set to place new worktrees")?;
    let placement = WorktreePlacement::new(home.join(WORKTREE_ROOT)).map_err(anyhow::Error::msg)?;
    let later: Arc<dyn LaterNotices> = match &args.app_home {
        Some(app_home) => Arc::new(NoticeFile::in_app_home(app_home)),
        None => Arc::new(LogNotices),
    };
    let host = CliAppHost::new(
        launch::find_program("herdr-app")?,
        launch::find_program("hc")?,
        args.app_home,
    );
    let git = Arc::new(CliGitProvider) as Arc<dyn GitProvider>;
    let mut backend = HerdrApp::new(
        Arc::new(host),
        Arc::clone(&git),
        placement,
        Arc::new(notifier),
        loaded.extension.on_open.clone(),
        background,
        later,
    );
    if let Some(spaces) = args.spaces {
        backend = backend.with_spaces(spaces);
    }
    Ok(kiosk_serve::Start {
        current_cwd: args.context.current_cwd().map(ToOwned::to_owned),
        loaded,
        git,
        backend: Some(Arc::new(backend)),
    })
}
