use std::fmt;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json as json;

use crate::config::Config;
use crate::types::Diff;
use crate::types::Page;
use crate::types::Task;

#[derive(Debug, Clone, Copy)]
pub enum Index {
    Diffs,
    Tasks,
    Pages,
}

pub struct Client<'a> {
    config: &'a Config,
    agent: ureq::Agent,
}

/// A search result from any index
#[derive(Debug, Clone)]
pub enum Hit {
    Diff(Diff),
    Task(Task),
    Page(Page),
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    q: &'a str,
    limit: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    filter: Option<&'a str>,
}

#[derive(Deserialize)]
struct SearchResponse<T> {
    hits: Vec<T>,
}

#[derive(Serialize)]
struct DiffWithRank<'a> {
    #[serde(flatten)]
    diff: &'a Diff,
    rank: u32,
}

#[derive(Serialize)]
struct TaskWithRank<'a> {
    #[serde(flatten)]
    task: &'a Task,
    rank: u32,
}

#[derive(Serialize)]
struct PageWithRank<'a> {
    #[serde(flatten)]
    page: &'a Page,
    rank: u32,
}

pub fn index_diffs(config: &Config, diffs: &[Diff]) -> Result<()> {
    if diffs.is_empty() {
        return Ok(());
    }
    let with_rank: Vec<_> = diffs
        .iter()
        .map(|diff| DiffWithRank {
            diff,
            rank: diff.rank(),
        })
        .collect();

    let client = Client::new_for_indexing(config);
    client.put_documents(Index::Diffs, &with_rank)?;
    client.patch_settings(Index::Diffs, Diff::settings())?;
    log::info!("meili: indexed {} diffs", diffs.len());
    Ok(())
}

pub fn index_tasks(config: &Config, tasks: &[Task]) -> Result<()> {
    if tasks.is_empty() {
        return Ok(());
    }
    let with_rank: Vec<_> = tasks
        .iter()
        .map(|task| TaskWithRank {
            task,
            rank: task.rank(),
        })
        .collect();

    let client = Client::new_for_indexing(config);
    client.put_documents(Index::Tasks, &with_rank)?;
    client.patch_settings(Index::Tasks, Task::settings())?;
    log::info!("meili: indexed {} tasks", tasks.len());
    Ok(())
}

pub fn index_pages(config: &Config, pages: &[Page]) -> Result<()> {
    if pages.is_empty() {
        return Ok(());
    }
    let with_rank: Vec<_> = pages
        .iter()
        .map(|page| PageWithRank {
            page,
            rank: page.rank(),
        })
        .collect();

    let client = Client::new_for_indexing(config);
    client.put_documents(Index::Pages, &with_rank)?;
    client.patch_settings(Index::Pages, Page::settings())?;
    log::info!("meili: indexed {} pages", pages.len());
    Ok(())
}

impl<'a> Client<'a> {
    pub fn new(config: &'a Config) -> Self {
        let agent = Self::make_agent(
            Duration::from_millis(300),
            Duration::from_millis(400),
            &config.meili_key,
        );
        Self { config, agent }
    }

    fn new_for_indexing(config: &'a Config) -> Self {
        let agent = Self::make_agent(
            Duration::from_secs(3),
            Duration::from_secs(20),
            &config.meili_key,
        );
        Self { config, agent }
    }

    fn make_agent(connect: Duration, recv_response: Duration, meili_key: &str) -> ureq::Agent {
        use ureq::http::Request;
        use ureq::http::header::HeaderValue;
        use ureq::middleware::MiddlewareNext;

        let auth = HeaderValue::try_from(format!("Bearer {meili_key}"))
            .expect("API key is a valid header value");

        ureq::config::Config::builder()
            .timeout_connect(Some(connect))
            .timeout_recv_response(Some(recv_response))
            .middleware(
                move |mut req: Request<ureq::SendBody<'_>>, next: MiddlewareNext<'_>| {
                    req.headers_mut().insert("Authorization", auth.clone());
                    next.handle(req)
                },
            )
            .build()
            .new_agent()
    }

    pub fn search_all(&self, q: &str, limit: usize) -> Result<Vec<Hit>> {
        let indices = Index::all();
        let per_index = (limit / indices.len()).max(1);
        let mut all = Vec::new();
        for index in indices {
            match self.search_one(index, q, None, per_index) {
                Ok(hits) => all.extend(hits),
                Err(err) => log::warn!("meilisearch search on {index} failed: {err:#}"),
            }
        }
        all.truncate(limit);
        Ok(all)
    }

    pub fn search_one(
        &self,
        index: Index,
        q: &str,
        filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        Ok(match index {
            Index::Diffs => self
                .search(index, q, filter, limit)?
                .into_iter()
                .map(Hit::Diff)
                .collect(),
            Index::Tasks => self
                .search(index, q, filter, limit)?
                .into_iter()
                .map(Hit::Task)
                .collect(),
            Index::Pages => self
                .search(index, q, filter, limit)?
                .into_iter()
                .map(Hit::Page)
                .collect(),
        })
    }

    fn search<T: DeserializeOwned>(
        &self,
        index: Index,
        q: &str,
        filter: Option<&str>,
        limit: usize,
    ) -> Result<Vec<T>> {
        let url = format!("{}/indexes/{index}/search", self.config.meili_url);
        let body = SearchRequest { q, limit, filter };
        let mut resp = self
            .agent
            .post(&url)
            .send_json(body)
            .context("meilisearch search failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("meilisearch search HTTP {status}: {body}");
        }
        let r: SearchResponse<T> = resp
            .body_mut()
            .read_json()
            .context("failed to parse search response")?;
        Ok(r.hits)
    }

    fn put_documents<T: Serialize>(&self, index: Index, docs: &[T]) -> Result<()> {
        let url = format!(
            "{}/indexes/{index}/documents?primaryKey=id",
            self.config.meili_url
        );
        let mut resp = self
            .agent
            .put(&url)
            .send_json(docs)
            .context("meilisearch PUT documents failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("meilisearch PUT documents HTTP {status}: {body}");
        }
        Ok(())
    }

    fn patch_settings(&self, index: Index, settings: json::Value) -> Result<()> {
        let url = format!("{}/indexes/{index}/settings", self.config.meili_url);
        let mut resp = self
            .agent
            .patch(&url)
            .send_json(settings)
            .context("meilisearch PATCH settings failed")?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            bail!("meilisearch PATCH settings HTTP {status}: {body}");
        }
        Ok(())
    }
}

impl fmt::Display for Index {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diffs => "ph_diffs",
            Self::Tasks => "ph_tasks",
            Self::Pages => "ph_pages",
        }
        .fmt(f)
    }
}

impl Index {
    fn all() -> [Self; 3] {
        [Self::Diffs, Self::Tasks, Self::Pages]
    }
}
