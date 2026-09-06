use crate::error::AppResult;
use keyring::Entry;

const SERVICE: &str = "replicator";
const USER: &str = "github-pat";

pub fn set_pat(token: &str) -> AppResult<()> {
    let entry = Entry::new(SERVICE, USER)?;
    entry.set_password(token)?;
    Ok(())
}

pub fn get_pat() -> AppResult<Option<String>> {
    let entry = Entry::new(SERVICE, USER)?;
    match entry.get_password() {
        Ok(t) => Ok(Some(t)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn clear_pat() -> AppResult<()> {
    let entry = Entry::new(SERVICE, USER)?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
