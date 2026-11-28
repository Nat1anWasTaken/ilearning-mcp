//! iLearning settings, read from the environment; the framework reads its own.

use anyhow::{Context, Result};
use mcp_backbone::config::optional;
use url::Url;

#[derive(Clone, Debug)]
pub struct ILearningConfig {
    /// The iLearning (Moodle) site root, with a trailing slash.
    pub ilearning_url: Url,
    /// Assumed lifetime of a freshly issued iLearning token, in days.
    pub token_duration_days: i64,
}

impl ILearningConfig {
    pub fn from_env() -> Result<Self> {
        let ilearning_url = site_url(
            &optional("ILEARNING_URL").unwrap_or_else(|| "https://ilearning.cycu.edu.tw".into()),
        )?;
        let token_duration_days = match optional("TOKEN_DURATION_DAYS") {
            Some(v) => v
                .trim()
                .parse()
                .ok()
                .filter(|days| *days > 0)
                .context("TOKEN_DURATION_DAYS must be a positive integer")?,
            None => 84,
        };
        Ok(Self {
            ilearning_url,
            token_duration_days,
        })
    }

    /// The assumed lifetime of a fresh iLearning token.
    pub fn token_duration(&self) -> chrono::TimeDelta {
        chrono::TimeDelta::days(self.token_duration_days)
    }
}

/// `url` as a site root, with a trailing slash.
pub fn site_url(url: &str) -> Result<Url> {
    let mut url = Url::parse(url).context("ILEARNING_URL is not a valid URL")?;
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::site_url;

    #[test]
    fn site_urls_end_with_a_slash() {
        assert_eq!(
            site_url("https://ilearning.example").unwrap().as_str(),
            "https://ilearning.example/"
        );
        assert_eq!(
            site_url("https://example.test/moodle").unwrap().as_str(),
            "https://example.test/moodle/"
        );
        assert!(site_url("not a url").is_err());
    }
}
