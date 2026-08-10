use std::iter;
use std::sync::LazyLock;
use std::time::Duration;
use std::time::UNIX_EPOCH;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use constcat::concat;
use powerpack::cache;
use powerpack::detach;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json as json;

use crate::config::Config;
use crate::meili;
use crate::types::Diff;
use crate::types::Page;
use crate::types::Repo;
use crate::types::Task;
use crate::types::User;

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

/// Spawns three background tasks to re-index diffs, tasks and wiki pages.
pub fn reindex(config: &Config) {
    if let Err(e) = diffs(config) {
        log::warn!("reindex diffs: {}", format_err(e));
    }
    if let Err(e) = tasks(config) {
        log::warn!("reindex tasks: {}", format_err(e));
    }
    if let Err(e) = pages(config) {
        log::warn!("reindex pages: {}", format_err(e));
    }
}

/// Fetches all diffs and reindexes them
fn diffs(config: &Config) -> Result<()> {
    let path = "/differential.revision.search";
    CACHE
        .query(
            cache::Query::new("diffs")
                .ttl(TTL_MINUTE)
                .checksum(checksum(config, path, &[]))
                .update_fn(move |prev| -> Result<()> {
                    let mut form = vec![("order", "updated".to_owned())];
                    if let Some(cache::PrevEntry {
                        entry: Ok(entry), ..
                    }) = prev
                    {
                        let start = entry
                            .pre_update_time
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64
                            - 600;
                        form.push(("constraints[modifiedStart]", start.to_string()));
                    };

                    let items = fetch_all(config, path, form)?;
                    let diffs: Vec<_> = items
                        .into_iter()
                        .map(|r| Diff::parse(r).context("failed to parse diff"))
                        .collect::<Result<_>>()?;
                    meili::index_diffs(config, &diffs)?;
                    Ok(())
                }),
        )
        .context("failed to fetch diffs from cache")?;
    Ok(())
}

impl Diff {
    fn parse(r: json::Value) -> Result<Self> {
        Ok(Self {
            id: lookup(&r, "/id").context("failed to extract `id`")?,
            title: lookup(&r, "/fields/title").context("failed to extract `title`")?,
            status: lookup(&r, "/fields/status/value").context("failed to extract `status`")?,
            author_id: lookup(&r, "/fields/authorPHID")
                .context("failed to extract `authorPHID`")?,
            updated_at: lookup(&r, "/fields/dateModified")
                .context("failed to extract `dateModified`")?,
        })
    }
}

/// Fetches all tasks and reindexes them
pub fn tasks(config: &Config) -> Result<()> {
    let path = "/maniphest.search";
    CACHE
        .query(
            cache::Query::new("tasks")
                .ttl(TTL_MINUTE)
                .checksum(checksum(config, path, &[]))
                .update_fn(|prev| -> Result<()> {
                    let mut form = vec![("order", "updated".to_owned())];
                    if let Some(cache::PrevEntry {
                        entry: Ok(entry), ..
                    }) = prev
                    {
                        let start = entry
                            .pre_update_time
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64
                            - 600;
                        form.push(("constraints[modifiedStart]", start.to_string()));
                    };

                    let items = fetch_all(config, path, form)?;
                    let tasks: Vec<_> = items
                        .into_iter()
                        .map(|r| Task::parse(r).context("failed to parse task"))
                        .collect::<Result<_>>()?;
                    meili::index_tasks(config, &tasks)?;
                    Ok(())
                }),
        )
        .context("failed to fetch tasks from cache")?;
    Ok(())
}

impl Task {
    fn parse(r: json::Value) -> Result<Self> {
        Ok(Self {
            id: lookup(&r, "/id").context("failed to extract `id`")?,
            title: lookup(&r, "/fields/name").context("failed to extract `name`")?,
            owner_id: lookup(&r, "/fields/ownerPHID").context("failed to extract `ownerPHID`")?,
            description: lookup(&r, "/fields/description/raw")
                .context("failed to extract `description`")?,
            status: lookup(&r, "/fields/status/value").context("failed to extract `status`")?,
            updated_at: lookup(&r, "/fields/dateModified")
                .context("failed to extract `dateModified`")?,
        })
    }
}

/// Fetches wiki pages
pub fn pages(config: &Config) -> Result<()> {
    let path = "/phriction.document.search";
    let form = [("order", "newest"), ("attachments[content]", "true")];
    CACHE
        .query(
            cache::Query::new("pages")
                .ttl(TTL_HOUR)
                .checksum(checksum(config, path, &[]))
                .update_fn(|_| -> Result<()> {
                    let items = fetch_all(config, path, form)?;
                    let pages: Vec<_> = items
                        .into_iter()
                        .filter_map(|r| {
                            Page::parse(r)
                                .transpose()
                                .map(|r| r.context("failed to parse page"))
                        })
                        .collect::<Result<_>>()?;
                    meili::index_pages(config, &pages)?;
                    Ok(())
                }),
        )
        .context("failed to fetch pages from cache")?;
    Ok(())
}

impl Page {
    fn parse(r: json::Value) -> Result<Option<Self>> {
        let typ: String = lookup(&r, "/type").unwrap_or_default();
        if typ != "WIKI" {
            return Ok(None);
        }
        Ok(Some(Self {
            id: lookup(&r, "/id").context("failed to extract `id`")?,
            path: lookup(&r, "/attachments/content/path").context("failed to extract `path`")?,
            status: lookup(&r, "/fields/status/value").context("failed to extract `status`")?,
            title: lookup(&r, "/attachments/content/title").context("failed to extract `title`")?,
            content: lookup(&r, "/attachments/content/content/raw")
                .context("failed to extract `content`")?,
        }))
    }
}

/// Fetches the repositories
pub fn repos(config: &Config) -> Result<Vec<Repo>> {
    let path = "/repository.query";
    let form = &[("order", "committed")];
    let result = CACHE
        .query(
            cache::Query::new("repos")
                .ttl(TTL_DAY)
                .checksum(checksum(config, path, form))
                .update_fn(|_| fetch(config, path, form)),
        )
        .context("failed to fetch repos from cache")?;

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

/// Fetches all the users
pub fn users(config: &Config) -> Result<Vec<User>> {
    let path = "/user.search";
    let form = [("order", "newest")];
    let items = CACHE
        .query(
            cache::Query::new("users")
                .ttl(TTL_DAY)
                .checksum(checksum(config, path, &form))
                .update_fn(move |_| fetch_all(config, path, form)),
        )
        .context("failed to fetch users from cache")?;

    let parse = |r| -> Result<User> {
        let handle: String =
            lookup(&r, "/fields/username").context("failed to extract `handle`")?;
        Ok(User {
            id: lookup(&r, "/phid").context("failed to extract `phid`")?,
            handle_lower: handle.to_lowercase(),
            handle,
        })
    };

    items
        .into_iter()
        .map(|r| parse(r).context("failed to parse user"))
        .collect()
}

fn checksum(config: &Config, path: &str, form: &[(&str, &str)]) -> [u8; 20] {
    use sha1::*;
    let mut hasher = Sha1::new();
    hasher.update(&config.ph_api_token);
    hasher.update(&config.ph_api_url);
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

fn fetch_all<I, V>(config: &Config, path: &str, form: I) -> Result<Vec<json::Value>>
where
    I: IntoIterator<Item = (&'static str, V)>,
    V: AsRef<str>,
{
    #[derive(Debug, Clone, Deserialize)]
    #[allow(dead_code)] // fields are used to assert deserialization
    struct Cursor {
        after: Option<String>,
        before: Option<String>,
        limit: u32,
        order: String,
    }

    let agent = AGENT_CONFIG.new_agent();
    let url = format!("{}{}", config.ph_api_url, path);

    let form: Vec<_> = form
        .into_iter()
        .map(|(k, v)| (k, v.as_ref().to_owned()))
        .collect();

    let mut items = Vec::new();
    let mut after_id: Option<String> = None;

    loop {
        let form = {
            let mut f = form.clone();
            f.push(("api.token", config.ph_api_token.clone()));
            if let Some(after_id) = after_id.take() {
                f.push(("after", after_id));
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

        let new_items = lookup::<Vec<json::Value>>(&data, "/result/data")
            .context("failed to extract `data`")?;
        log::info!("ph: fetched {} ({} new items)", url, new_items.len());
        items.extend(new_items);

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
    let url = format!("{}{}", config.ph_api_url, path);

    let form = iter::once(("api.token", config.ph_api_token.as_str())).chain(form.iter().copied());

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
    log::info!("ph: fetched {url}");

    lookup::<json::Value>(&data, "/result").context("failed to extract `result`")
}

fn check_error(data: &json::Value) -> Result<()> {
    if let Some(err_code) = data.pointer("/error_code")
        && !err_code.is_null()
    {
        let err_info: String = lookup(data, "/error_info").unwrap_or_default();
        bail!("error code {}: {}", err_code, err_info);
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

fn format_err(err: anyhow::Error) -> String {
    detach::format_err(&*err.into_boxed_dyn_error())
}
