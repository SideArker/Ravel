use anyhow::{Context, Result};
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

const LATEST_RELEASE: &str = "https://api.github.com/repos/SideArker/Ravel/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

pub fn check() -> Result<Option<Version>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    let mut response = agent
        .get(LATEST_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("Ravel/", env!("CARGO_PKG_VERSION")))
        .call()
        .context("GitHub release request failed")?;
    let release: Release = response
        .body_mut()
        .with_config()
        .limit(64 * 1024)
        .read_json()
        .context("Invalid GitHub release response")?;
    newer_tag(&release.tag_name, env!("CARGO_PKG_VERSION"))
}

fn newer_tag(tag: &str, current: &str) -> Result<Option<Version>> {
    let latest = Version::parse(tag.strip_prefix('v').unwrap_or(tag))
        .context("Invalid GitHub release version")?;
    let current = Version::parse(current).context("Invalid installed version")?;
    Ok((latest > current).then_some(latest))
}

pub fn release_url(version: &Version) -> String {
    format!("https://github.com/SideArker/Ravel/releases/tag/v{version}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_newer_semantic_versions_prompt() {
        assert_eq!(
            newer_tag("v0.2.0", "0.1.1").unwrap().unwrap().to_string(),
            "0.2.0"
        );
        assert!(newer_tag("v0.1.1", "0.1.1").unwrap().is_none());
        assert!(newer_tag("v0.1.0", "0.1.1").unwrap().is_none());
        assert!(newer_tag("vnot-a-version", "0.1.1").is_err());
        assert_eq!(
            release_url(&Version::parse("0.2.0").unwrap()),
            "https://github.com/SideArker/Ravel/releases/tag/v0.2.0"
        );
    }

    #[test]
    #[ignore = "requires GitHub network"]
    fn github_latest_release_is_valid() {
        check().unwrap();
    }
}
