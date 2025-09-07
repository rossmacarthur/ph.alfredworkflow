use std::collections::HashMap;
use std::fs;

use anyhow::{Context as _, Result};
use serde::Deserialize;
use serde_json as json;

#[derive(Debug)]
pub struct Config {
    pub api_url: String,
    pub api_token: String,
}

impl Config {
    pub fn load() -> Result<Self> {
        #[derive(Debug, Deserialize)]
        struct ArcRc {
            config: ArcRcConfig,
            hosts: HashMap<String, ArcRcHost>,
        }

        #[derive(Debug, Deserialize)]
        struct ArcRcConfig {
            default: String,
        }

        #[derive(Debug, Deserialize)]
        struct ArcRcHost {
            token: String,
        }

        let home = {
            let mut path = home::home_dir().context("could not find home directory")?;
            path.push(".arcrc");
            path
        };

        let mut rc: ArcRc = {
            let contents =
                fs::read(&home).with_context(|| format!("failed to read {}", home.display()))?;
            json::from_slice(&contents).context("failed to parse JSON")?
        };
        rc.config.default.push_str("api/");

        let api_url = rc.config.default;

        let host = rc
            .hosts
            .remove(&api_url)
            .context("failed to find host config for default url")?;

        let api_token = host.token;

        Ok(Self { api_url, api_token })
    }
}
