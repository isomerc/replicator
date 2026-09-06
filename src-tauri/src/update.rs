use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};

/// One-shot "is a newer release out?" check against GitHub, same
/// contract as nicotine's: the UI shows a green LATEST VERSION when
/// up to date, a red link to the release when behind, and nothing at
/// all when the check errors (offline, private repo, rate limit).
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/isomerc/replicator/releases/latest";

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize)]
pub struct UpdateCheck {
    pub current: String,
    pub latest: String,
    pub url: String,
    pub outdated: bool,
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

pub async fn fetch_update_check() -> AppResult<UpdateCheck> {
    // GitHub's API rejects anonymous requests without a User-Agent;
    // reuse the ESI one so all our traffic identifies the same way.
    let client = reqwest::Client::builder()
        .user_agent(crate::eve::esi::USER_AGENT)
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    let res = client.get(LATEST_RELEASE_API).send().await?;
    if !res.status().is_success() {
        return Err(AppError::Other(format!(
            "GitHub API returned {}",
            res.status()
        )));
    }
    let release: GithubRelease = res.json().await?;
    evaluate(&release.tag_name, &release.html_url, CURRENT_VERSION)
}

/// Pure comparison core: tag from GitHub vs the running version.
fn evaluate(tag_name: &str, html_url: &str, current: &str) -> AppResult<UpdateCheck> {
    let latest = tag_name.trim_start_matches('v');
    let outdated = parse_version(latest)? > parse_version(current)?;
    Ok(UpdateCheck {
        current: current.to_string(),
        latest: latest.to_string(),
        url: html_url.to_string(),
        outdated,
    })
}

/// "0.2.1" -> (0, 2, 1). Tuples compare lexicographically, which is
/// exactly semver precedence for plain major.minor.patch.
fn parse_version(version: &str) -> AppResult<(u32, u32, u32)> {
    let parts: Vec<&str> = version.split('.').collect();
    let [major, minor, patch] = parts[..] else {
        return Err(AppError::Other(format!(
            "unexpected version format: {version}"
        )));
    };
    let parse = |s: &str| {
        s.parse::<u32>()
            .map_err(|_| AppError::Other(format!("unexpected version format: {version}")))
    };
    Ok((parse(major)?, parse(minor)?, parse(patch)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_into_ordered_tuples() {
        assert_eq!(parse_version("0.2.1").unwrap(), (0, 2, 1));
        assert_eq!(parse_version("10.20.30").unwrap(), (10, 20, 30));
        assert!(parse_version("1.2").is_err());
        assert!(parse_version("1.2.3.4").is_err());
        assert!(parse_version("v1.2.3").is_err());
        assert!(parse_version("one.two.three").is_err());
    }

    #[test]
    fn a_higher_tag_is_outdated_and_the_v_prefix_is_stripped() {
        let c = evaluate("v0.2.0", "https://example.com/r", "0.1.0").unwrap();
        assert!(c.outdated);
        assert_eq!(c.latest, "0.2.0");
        assert_eq!(c.current, "0.1.0");
        assert_eq!(c.url, "https://example.com/r");
    }

    #[test]
    fn equal_and_older_tags_are_up_to_date() {
        // Older covers the dev case: running an unreleased bump.
        assert!(!evaluate("v0.1.0", "u", "0.1.0").unwrap().outdated);
        assert!(!evaluate("v0.1.0", "u", "0.2.0").unwrap().outdated);
        // Numeric compare, not string compare.
        assert!(evaluate("v0.10.0", "u", "0.9.9").unwrap().outdated);
    }

    #[test]
    fn a_garbage_tag_errors_rather_than_reporting_a_state() {
        // The UI treats an error as "render nothing", which is the
        // right face for a tag we cannot interpret.
        assert!(evaluate("nightly", "u", "0.1.0").is_err());
    }

    #[test]
    fn github_release_matches_the_releases_api_shape() {
        // Pins the two fields we rely on from
        // /repos/{owner}/{repo}/releases/latest.
        let body = r#"{
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/isomerc/replicator/releases/tag/v0.2.0",
            "name": "0.2.0",
            "draft": false
        }"#;
        let r: GithubRelease = serde_json::from_str(body).unwrap();
        assert_eq!(r.tag_name, "v0.2.0");
        assert!(r.html_url.ends_with("/v0.2.0"));
    }

    #[test]
    fn cargo_and_tauri_conf_agree_on_the_version() {
        // The badge compares GitHub tags against CARGO_PKG_VERSION,
        // but release artifacts are stamped from tauri.conf.json. If
        // the two drift the badge lies to every user; pin them equal.
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["version"].as_str().unwrap(), CURRENT_VERSION);
    }
}
