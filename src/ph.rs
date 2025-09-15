use std::iter;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use constcat::concat;
use powerpack::cache;
use serde::{Deserialize, de::DeserializeOwned};
use serde_json as json;

use crate::config::Config;

const USER_AGENT: &str = concat!(crate::PKG_NAME, "/", crate::PKG_VERSION);

const TTL_MINUTE: Duration = Duration::from_secs(60);
const TTL_HOUR: Duration = Duration::from_secs(60 * 60);
const TTL_DAY: Duration = Duration::from_secs(24 * 60 * 60);

static CACHE: LazyLock<cache::Cache> = LazyLock::new(|| {
    cache::Builder::new()
        .ttl(TTL_HOUR)
        .initial_poll(Duration::from_millis(500))
        .build()
});

#[derive(Debug, Clone)]
pub struct User {
    pub phid: String,
    pub handle: String,
    pub real_name: String,
}

/// Fetches all the users.
pub fn users(config: &Config) -> Result<Vec<User>> {
    let path = "/user.search";
    let form = &[("order", "newest")];
    let items = CACHE.query(
        cache::Query::new("users")
            .ttl(TTL_DAY)
            .checksum(checksum(config, path, form))
            .update_fn(|| fetch_all(config, path, form)),
    )?;

    let parse = |r| -> Result<User> {
        Ok(User {
            phid: lookup(&r, "/phid").context("failed to extract `phid`")?,
            handle: lookup(&r, "/fields/username").context("failed to extract `username`")?,
            real_name: lookup(&r, "/fields/realName").context("failed to extract `realName`")?,
        })
    };

    items
        .into_iter()
        .map(|r| parse(r).context("failed to parse user"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub description: Option<String>,
    pub uri: String,
}

/// Fetches the repositories.
pub fn repos(config: &Config) -> Result<Vec<Repo>> {
    let path = "/repository.query";
    let form = &[("order", "committed")];
    let result = CACHE.query(
        cache::Query::new("repos")
            .ttl(TTL_DAY)
            .checksum(checksum(config, path, form))
            .update_fn(|| fetch(config, path, form)),
    )?;

    let parse = |r| -> Result<Repo> {
        Ok(Repo {
            name: lookup(&r, "/name").context("failed to extract `name`")?,
            description: lookup(&r, "/description").context("failed to extract `description`")?,
            uri: lookup(&r, "/uri").context("failed to extract `uri`")?,
        })
    };

    result
        .as_array()
        .context("expected array of repos")?
        .to_owned()
        .into_iter()
        .map(|r| parse(r).context("failed to parse repo"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Diff {
    pub id_title: String,
    pub uri: String,
    pub status: String,
    pub author_phid: String,
    pub updated: jiff::Timestamp,
}

/// Fetches the open diffs
pub fn diffs(config: &Config) -> Result<Vec<Diff>> {
    let path = "/differential.revision.search";
    let form = &[
        ("constraints[statuses][]", "needs-review"),
        ("constraints[statuses][]", "needs-revision"),
        ("constraints[statuses][]", "changes-planned"),
        ("constraints[statuses][]", "accepted"),
        ("order", "updated"),
    ];
    let items = CACHE.query(
        cache::Query::new("diffs")
            .ttl(TTL_MINUTE)
            .checksum(checksum(config, path, form))
            .update_fn(|| fetch_all(config, path, form)),
    )?;

    let parse = |r| -> Result<Diff> {
        let id: u32 = lookup(&r, "/id").context("failed to extract `id`")?;
        let title: String = lookup(&r, "/fields/title").context("failed to extract `title`")?;
        let id_title = format!("D{}: {}", id, title);
        Ok(Diff {
            id_title,
            uri: lookup(&r, "/fields/uri").context("failed to extract `uri`")?,
            status: lookup(&r, "/fields/status/value").context("failed to extract `status`")?,
            author_phid: lookup(&r, "/fields/authorPHID")
                .context("failed to extract `authorPHID`")?,
            updated: jiff::Timestamp::from_second(
                lookup(&r, "/fields/dateModified").context("failed to extract `dateModified`")?,
            )?,
        })
    };

    items
        .into_iter()
        .map(|r| parse(r).context("failed to parse diff"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id_title: String,
    pub uri: String,
    pub owner_phid: Option<String>,
    pub updated: jiff::Timestamp,
}

/// Fetches the open tasks
pub fn tasks(config: &Config) -> Result<Vec<Task>> {
    let path = "/maniphest.search";
    let form = &[("constraints[statuses][]", "open"), ("order", "updated")];

    let result = CACHE.query(
        cache::Query::new("tasks")
            .ttl(TTL_HOUR)
            .checksum(checksum(config, path, form))
            .update_fn(|| fetch_all(config, path, form)),
    )?;

    let parse = |r| -> Result<Task> {
        let id: u32 = lookup(&r, "/id").context("failed to extract `id`")?;
        let title: String = lookup(&r, "/fields/name").context("failed to extract `name`")?;
        let id_title = format!("T{}: {}", id, title);
        let uri = format!("{}/T{}", config.api_url.trim_end_matches("/api/"), id);
        let owner_phid: Option<String> =
            lookup(&r, "/fields/ownerPHID").context("failed to extract `ownerPHID`")?;
        let updated = jiff::Timestamp::from_second(
            lookup(&r, "/fields/dateModified").context("failed to extract `dateModified`")?,
        )?;
        Ok(Task {
            id_title,
            uri,
            owner_phid,
            updated,
        })
    };

    result
        .into_iter()
        .map(|r| parse(r).context("failed to parse task"))
        .collect()
}

#[derive(Debug, Clone)]
pub struct Document {
    pub title: String,
    pub path: String,
    pub content: String,
}

/// Fetches wiki documents
pub fn documents(config: &Config) -> Result<Vec<Document>> {
    let path = "/phriction.document.search";
    let form = &[("order", "newest"), ("attachments[content]", "true")];
    let items = CACHE.query(
        cache::Query::new("documents")
            .ttl(TTL_DAY)
            .checksum(checksum(config, path, form))
            .update_fn(|| fetch_all(config, path, form)),
    )?;

    let parse = |r| -> Result<Document> {
        Ok(Document {
            title: lookup(&r, "/attachments/content/title").context("failed to extract `title`")?,
            path: lookup(&r, "/attachments/content/path").context("failed to extract `path`")?,
            content: lookup(&r, "/attachments/content/content/raw")
                .context("failed to extract `content`")?,
        })
    };

    items
        .into_iter()
        .filter(|r| {
            let typ: String = lookup(r, "/type").unwrap_or_default();
            let status: String = lookup(r, "/fields/status/value").unwrap_or_default();
            typ == "WIKI" && status == "active"
        })
        .map(|r| parse(r).context("failed to parse document"))
        .collect()
}

fn checksum(config: &Config, path: &str, form: &[(&str, &str)]) -> [u8; 20] {
    use sha1::*;
    let mut hasher = Sha1::new();
    hasher.update(&config.api_token);
    hasher.update(&config.api_url);
    hasher.update(path);
    for (k, v) in form {
        hasher.update(k);
        hasher.update(v);
    }
    hasher.finalize().into()
}

static AGENT_CONFIG: LazyLock<ureq::config::Config> = LazyLock::new(|| {
    ureq::config::Config::builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .build(),
        )
        .build()
});

fn fetch_all(config: &Config, path: &str, form: &[(&str, &str)]) -> Result<Vec<json::Value>> {
    #[derive(Debug, Clone, Deserialize)]
    #[allow(dead_code)] // fields are used to assert deserialization
    struct Cursor {
        after: Option<String>,
        before: Option<String>,
        limit: u32,
        order: String,
    }

    let agent = AGENT_CONFIG.new_agent();
    let url = format!("{}{}", config.api_url, path);

    let mut items = Vec::new();
    let mut after_id: Option<String> = None;

    loop {
        let form = {
            let mut f = form.to_vec();
            f.push(("api.token", config.api_token.as_str()));
            if let Some(ref after_id) = after_id {
                f.push(("after", after_id.as_str()));
            }
            f
        };

        let mut resp = agent
            .post(&url)
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .send_form(form)?;
        if !resp.status().is_success() {
            bail!("HTTP {}", resp.status());
        }

        let data: json::Value = resp.body_mut().read_json()?;
        check_error(&data)?;

        items.extend(
            lookup::<Vec<json::Value>>(&data, "/result/data")
                .context("failed to extract `data`")?,
        );

        let cursor =
            lookup::<Cursor>(&data, "/result/cursor").context("failed to extract `cursor`")?;
        match cursor.after {
            Some(item_id) => after_id = Some(item_id),
            None => break Ok(items),
        }
    }
}

fn fetch(config: &Config, path: &str, form: &[(&str, &str)]) -> Result<json::Value> {
    let agent = AGENT_CONFIG.new_agent();
    let url = format!("{}{}", config.api_url, path);

    let form = iter::once(("api.token", config.api_token.as_str())).chain(form.iter().copied());

    let mut resp = agent
        .post(&url)
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .send_form(form)?;
    if !resp.status().is_success() {
        bail!("HTTP {}", resp.status());
    }

    let data: json::Value = resp.body_mut().read_json()?;
    check_error(&data)?;

    lookup::<json::Value>(&data, "/result").context("failed to extract `result`")
}

fn check_error(data: &json::Value) -> Result<()> {
    if let Some(err_code) = data.pointer("/error_code")
        && !err_code.is_null()
    {
        let err_info: String = lookup(data, "/error_info").unwrap_or_default();
        bail!("Error code {}: {}", err_code, err_info);
    }
    Ok(())
}

fn lookup<T>(value: &json::Value, ptr: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let v = value
        .pointer(ptr)
        .with_context(|| format!("failed to lookup `{ptr}` in `{value:?}`"))?;
    Ok(json::from_value(v.clone())?)
}
