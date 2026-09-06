use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

const ESI_NAMES_URL: &str = "https://esi.evetech.net/latest/universe/names/";
pub const USER_AGENT: &str = concat!(
    "Replicator/",
    env!("CARGO_PKG_VERSION"),
    " (+https://replicator.rip)"
);

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct EsiName {
    pub id: i64,
    pub name: String,
    pub category: String,
}

/// What a resolve pass learned: the names ESI returned, plus the ids
/// it refused to resolve at all (deleted/biomassed characters, whose
/// `.dat` files outlive them on disk).
#[derive(Debug, Default)]
pub struct EsiResolution {
    pub names: Vec<EsiName>,
    pub unresolvable: Vec<i64>,
}

pub async fn resolve_ids(ids: &[i64]) -> AppResult<EsiResolution> {
    let mut out = EsiResolution::default();
    if ids.is_empty() {
        return Ok(out);
    }
    // reqwest's default is NO timeout; offline must mean a fast error,
    // not a resolve that never settles.
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    // /universe/names/ 404s the WHOLE payload when any single id is
    // unknown, and unknown ids are routine here (deleted characters
    // leave their files behind forever). On a 404, bisect: split the
    // chunk and retry the halves, isolating the dead ids at log2 cost
    // while every good id still resolves.
    let mut queue: Vec<Vec<i64>> = ids.chunks(900).map(|c| c.to_vec()).collect();
    while let Some(chunk) = queue.pop() {
        let res = client.post(ESI_NAMES_URL).json(&chunk).send().await?;
        if res.status() == reqwest::StatusCode::NOT_FOUND {
            if let [only] = chunk[..] {
                out.unresolvable.push(only);
            } else {
                let mid = chunk.len() / 2;
                queue.push(chunk[..mid].to_vec());
                queue.push(chunk[mid..].to_vec());
            }
            continue;
        }
        if !res.status().is_success() {
            let status = res.status();
            let body = res.text().await.unwrap_or_default();
            return Err(AppError::Other(format!(
                "ESI {} returned {status}: {body}",
                ESI_NAMES_URL
            )));
        }
        let names: Vec<EsiName> = res.json().await?;
        out.names.extend(names);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolving_an_empty_id_list_short_circuits_before_any_request() {
        // list_characters calls this on every refresh; with a fully
        // cached name table it must not hit the network at all.
        let r = resolve_ids(&[]).await.unwrap();
        assert!(r.names.is_empty());
        assert!(r.unresolvable.is_empty());
    }

    #[test]
    fn esi_name_matches_the_universe_names_response_shape() {
        // Pins the contract with https://esi.evetech.net/latest/universe/names/
        let body = r#"[
            {"category":"character","id":2035047876,"name":"Alice Smith"},
            {"category":"corporation","id":98000001,"name":"Some Corp"}
        ]"#;

        let names: Vec<EsiName> = serde_json::from_str(body).unwrap();

        assert_eq!(names.len(), 2);
        assert_eq!(names[0].id, 2035047876);
        assert_eq!(names[0].name, "Alice Smith");
        assert_eq!(names[0].category, "character");
    }

    #[test]
    fn a_response_missing_a_field_is_rejected_rather_than_defaulted() {
        let body = r#"[{"id":123,"name":"NoCategory"}]"#;
        assert!(serde_json::from_str::<Vec<EsiName>>(body).is_err());
    }

    #[test]
    fn the_user_agent_identifies_the_app_and_a_contact() {
        // Fenris Creations throttles or blocks anonymous ESI clients.
        assert!(USER_AGENT.contains("Replicator"));
        assert!(USER_AGENT.contains("https://replicator.rip"));
        // Stamped from the package version, so it cannot go stale.
        assert!(USER_AGENT.contains(env!("CARGO_PKG_VERSION")));
    }
}
