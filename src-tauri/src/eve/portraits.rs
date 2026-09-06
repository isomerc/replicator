//! Character portraits from the public EVE image server, cached on disk.
//!
//! The webview's CSP allows img-src 'self' and data: only, on purpose -
//! the frontend never talks to the network. Portraits are fetched here,
//! written to the app data dir once, and handed over as data: URIs, so
//! they cost one download ever and keep working offline.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 128px covers the album cell at 2x for hidpi.
const SIZE: u32 = 128;

/// Best-effort: returns a data URI per id it could produce, from cache
/// or the image server. Ids that fail (offline, deleted character,
/// server trouble) are simply absent - the UI falls back to the
/// monogram plate.
pub async fn fetch_portraits(cache_dir: &Path, ids: &[i64]) -> HashMap<i64, String> {
    let mut out = HashMap::new();
    let _ = std::fs::create_dir_all(cache_dir);

    let mut missing = Vec::new();
    for &id in ids {
        match std::fs::read(cache_path(cache_dir, id)) {
            Ok(bytes) => {
                out.insert(id, to_data_uri(&bytes));
            }
            Err(_) => missing.push(id),
        }
    }
    if missing.is_empty() {
        return out;
    }

    let client = match reqwest::Client::builder()
        .user_agent(crate::eve::esi::USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(_) => return out,
    };

    let mut set = tokio::task::JoinSet::new();
    for id in missing {
        let client = client.clone();
        set.spawn(async move {
            let url = format!("https://images.evetech.net/characters/{id}/portrait?size={SIZE}");
            let res = client.get(&url).send().await.ok()?;
            if !res.status().is_success() {
                return None;
            }
            let bytes = res.bytes().await.ok()?;
            Some((id, bytes.to_vec()))
        });
    }
    while let Some(joined) = set.join_next().await {
        if let Ok(Some((id, bytes))) = joined {
            let _ = std::fs::write(cache_path(cache_dir, id), &bytes);
            out.insert(id, to_data_uri(&bytes));
        }
    }
    out
}

fn cache_path(dir: &Path, id: i64) -> PathBuf {
    dir.join(format!("{id}.jpg"))
}

fn to_data_uri(bytes: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", STANDARD.encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn data_uris_are_well_formed() {
        assert_eq!(
            to_data_uri(&[0xFF, 0xD8, 0xFF]),
            "data:image/jpeg;base64,/9j/"
        );
    }

    #[tokio::test]
    async fn cached_portraits_are_served_without_any_network() {
        // Every requested id is on disk, so the function must return
        // before a client is even built - this test passing offline is
        // the proof.
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("1001.jpg"), [0xFF, 0xD8]).unwrap();

        let got = fetch_portraits(tmp.path(), &[1001]).await;

        assert_eq!(got.get(&1001).unwrap(), "data:image/jpeg;base64,/9g=");
    }

    #[tokio::test]
    async fn an_empty_id_list_is_an_empty_map() {
        let tmp = TempDir::new().unwrap();
        assert!(fetch_portraits(tmp.path(), &[]).await.is_empty());
    }
}
