use std::collections::HashMap;
use std::fs;

use anyhow::Context as _;
use anyhow::Result;
use powerpack::env;
use serde::Deserialize;
use serde_json as json;

#[derive(Debug)]
pub struct Config {
    pub ph_base_url: String,
    pub ph_api_url: String,
    pub ph_api_token: String,
    pub meili_url: String,
    pub meili_key: String,
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

        let rc: ArcRc = {
            let contents =
                fs::read(&home).with_context(|| format!("failed to read {}", home.display()))?;
            json::from_slice(&contents).context("failed to parse JSON")?
        };

        let ph_base_url = rc.config.default.trim_end_matches('/').to_owned();

        let (ph_api_url, ph_api_token) = rc
            .hosts
            .into_iter()
            .next()
            .map(|(ph_api_url, rc_host)| {
                (ph_api_url.trim_end_matches('/').to_owned(), rc_host.token)
            })
            .context("failed to find host config")?;

        let meili_url =
            env::var("MEILI_URL").unwrap_or_else(|| "http://localhost:7700".to_string());
        let meili_key = env::var("MEILI_KEY").unwrap_or_default();

        Ok(Self {
            ph_base_url,
            ph_api_url,
            ph_api_token,
            meili_url,
            meili_key,
        })
    }
}
