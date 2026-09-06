use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("db: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("git: {0}")]
    Git(#[from] git2::Error),

    #[error("http: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("keyring: {0}")]
    Keyring(#[from] keyring::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("eve is currently running; close all clients before this operation")]
    EveRunning,

    #[error("{0}")]
    Other(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        s.serialize_str(&self.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// Errors reach the UI as a bare JSON string - `invoke()` rejects
    /// with it and the frontend renders it via `String(e)` straight into
    /// a banner. These messages are the whole error UX.
    fn as_json(e: AppError) -> String {
        serde_json::to_string(&e).unwrap()
    }

    #[test]
    fn eve_running_explains_what_to_do() {
        let msg = AppError::EveRunning.to_string();
        assert_eq!(
            msg,
            "eve is currently running; close all clients before this operation"
        );
    }

    #[test]
    fn errors_serialize_to_a_plain_json_string_not_an_object() {
        // A tagged enum here would render as "[object Object]" in the
        // banner instead of the message.
        assert_eq!(
            as_json(AppError::Config("no remote".into())),
            "\"config: no remote\""
        );
        assert_eq!(
            as_json(AppError::NotFound("group 7".into())),
            "\"not found: group 7\""
        );
        assert_eq!(as_json(AppError::Other("boom".into())), "\"boom\"");
    }

    #[test]
    fn each_variant_is_prefixed_so_the_user_can_tell_them_apart() {
        let io: AppError = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert_eq!(io.to_string(), "io: gone");

        let db: AppError = rusqlite::Error::QueryReturnedNoRows.into();
        assert!(db.to_string().starts_with("db: "));

        let json: AppError = serde_json::from_str::<i32>("nope").unwrap_err().into();
        assert!(json.to_string().starts_with("json: "));
    }

    #[test]
    fn io_errors_convert_via_the_question_mark_operator() {
        fn read_missing() -> AppResult<String> {
            Ok(std::fs::read_to_string("/nonexistent/replicator/test")?)
        }
        assert!(matches!(read_missing().unwrap_err(), AppError::Io(_)));
    }
}
