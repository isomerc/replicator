use crate::db::Db;
use crate::error::{AppError, AppResult};
use crate::eve::paths::EveInstall;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub struct AppState {
    pub db: Db,
    pub mirror_dir: PathBuf,
    pub portrait_dir: PathBuf,
    pub installs: Vec<EveInstall>,
    pub manual_override: Option<PathBuf>,
}

impl AppState {
    pub fn initialize(app: &AppHandle) -> AppResult<Self> {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| AppError::Config(format!("app_data_dir: {e}")))?;
        std::fs::create_dir_all(&data_dir)?;

        let db_path = data_dir.join("replicator.sqlite");
        let db = Db::open(&db_path)?;

        let mirror_dir = data_dir.join("mirror");
        std::fs::create_dir_all(&mirror_dir)?;

        let portrait_dir = data_dir.join("portraits");

        let manual_override = db
            .get_setting("install_override")?
            .map(PathBuf::from)
            .filter(|p| p.exists());

        let installs = crate::eve::paths::discover(manual_override.as_deref());

        Ok(Self {
            db,
            mirror_dir,
            portrait_dir,
            installs,
            manual_override,
        })
    }

    pub fn rescan(&mut self) {
        self.installs = crate::eve::paths::discover(self.manual_override.as_deref());
    }
}
