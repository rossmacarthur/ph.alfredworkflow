mod config;
mod human;
mod meili;
mod ph;
mod types;

use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::io;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use constcat::concat;
use powerpack::Icon;
use powerpack::Item;
use powerpack::cache;
use powerpack::logger;

use crate::config::Config;
use crate::types::Diff;
use crate::types::Page;
use crate::types::Repo;
use crate::types::Task;
use crate::types::User;

const PKG_NAME: &str = env!("CARGO_PKG_NAME");
const PKG_VERSION: &str = env!("CARGO_PKG_VERSION");
const LOG_FILENAME: &str = concat!(PKG_NAME, "-", PKG_VERSION, ".log");

struct Context {
    config: Config,
    users: HashMap<String, User>,
    now: jiff::Timestamp,
}

#[derive(Debug, Clone, Copy)]
enum Command {
    Diffs,
    Tasks,
    Pages,
    Repos,
    Search,
}

fn main() -> Result<()> {
    if let Err(err) = run() {
        eprintln!("ERROR: {err:#}");
        let item = if let Some(cache::QueryError::Miss) = err.downcast_ref::<cache::QueryError>() {
            Item::new(format!("Warning: {err}"))
                .subtitle("The workflow is still loading data from Phabricator")
                .icon(Icon::with_image("./assets/warning.png"))
        } else {
            Item::new(format!("Error: {err}"))
                .subtitle(
                    "The workflow errored! You might want to try debugging it or checking the logs",
                )
                .icon(Icon::with_image("./assets/error.png"))
        };
        output([item])?;
    }
    Ok(())
}

fn run() -> Result<()> {
    logger::Builder::new().filename(LOG_FILENAME).try_init()?;
    let config = Config::load()?;
    let users = ph::users(&config)?
        .into_iter()
        .map(|u| (u.id.clone(), u))
        .collect();
    let ctx = Context {
        config,
        users,
        now: jiff::Timestamp::now(),
    };

    let arg = env::args()
        .nth(1)
        .as_deref()
        .map(str::trim)
        .map(str::to_lowercase);

    let cmds = Command::all();

    let items = match arg {
        // If no argument is given then just list the available commands
        None => cmds.into_iter().map(Command::into_item).collect(),

        // Otherwise process the argument
        Some(arg) => {
            // Get the command and the search query
            let (cmd, query) = arg.split_once(char::is_whitespace).unwrap_or((&arg, ""));

            match cmds.iter().find(|c| c.name() == cmd) {
                // There is a command that matches this query so execute it
                Some(cmd) => cmd.exec(&ctx, query)?,

                // No command matches the query exactly, output the commands
                // that start with the half-entered command
                None => {
                    let items: Vec<_> = cmds
                        .into_iter()
                        .filter(|c| c.name().starts_with(cmd))
                        .map(Command::into_item)
                        .collect();
                    if items.is_empty() {
                        let item = Item::new(format!("No command found: '{cmd}'"));
                        return output([item]);
                    }
                    items
                }
            }
        }
    };

    output(items)
}

impl Command {
    fn all() -> [Self; 5] {
        [
            Self::Diffs,
            Self::Tasks,
            Self::Pages,
            Self::Repos,
            Self::Search,
        ]
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Diffs => "diffs",
            Self::Tasks => "tasks",
            Self::Pages => "wiki",
            Self::Repos => "repos",
            Self::Search => "search",
        }
    }

    fn subtitle(&self) -> &'static str {
        match self {
            Self::Diffs => "Search differential revisions",
            Self::Tasks => "Search maniphest tasks",
            Self::Pages => "Search phriction pages",
            Self::Repos => "Search repositories",
            Self::Search => "Search all available content",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::Diffs => "./assets/diff.png",
            Self::Tasks => "./assets/task.png",
            Self::Pages => "./assets/wiki.png",
            Self::Repos => "./assets/repo.png",
            Self::Search => "./assets/search.png",
        }
    }

    fn into_item(self) -> Item {
        let name = self.name();
        Item::new(name)
            .subtitle(self.subtitle())
            .autocomplete(format!("{name} "))
            .icon(Icon::with_image(self.icon()))
    }

    fn exec(&self, ctx: &Context, query: &str) -> Result<Vec<Item>> {
        ph::reindex(&ctx.config);
        let items: Vec<_> = match self {
            Self::Diffs | Self::Tasks | Self::Pages => {
                let index = match self {
                    Self::Diffs => meili::Index::Diffs,
                    Self::Tasks => meili::Index::Tasks,
                    Self::Pages => meili::Index::Pages,
                    _ => unreachable!(),
                };
                let (text, filter) = match build_query_filter(ctx, index, query) {
                    Some(f) => f,
                    None => return Ok(vec![]),
                };
                meili::Client::new(&ctx.config)
                    .search_one(index, &text, filter.as_deref(), 20)
                    .context("meilisearch search failed")?
                    .into_iter()
                    .map(|h| h.into_item(ctx))
                    .collect()
            }

            Self::Repos => ph::repos(&ctx.config)?
                .into_iter()
                .filter(|r| r.matches(query))
                .map(|r| r.into_item())
                .take(20)
                .collect(),

            Self::Search => meili::Client::new(&ctx.config)
                .search_all(query, 20)?
                .into_iter()
                .map(|h| h.into_item(ctx))
                .collect(),
        };
        Ok(items)
    }
}

fn build_query_filter(
    ctx: &Context,
    index: meili::Index,
    query: &str,
) -> Option<(String, Option<String>)> {
    let (handles, statuses, not_statuses, text) = parse_query(query);

    let mut conditions = Vec::new();

    if let Some(user_field) = index.user_field()
        && !handles.is_empty()
    {
        let matching = ctx.handles_matching(&handles);
        if matching.is_empty() {
            return None;
        }
        if let Some(cond) = filter_contains(user_field, &matching, false) {
            conditions.push(cond);
        }
    }

    if let Some(status_field) = index.status_field()
        && !statuses.is_empty()
    {
        let matching = index.statuses_matching(&statuses);
        if matching.is_empty() {
            return None;
        }
        if let Some(cond) = filter_contains(status_field, &matching, false) {
            conditions.push(cond);
        }
    }

    if let Some(status_field) = index.status_field()
        && !not_statuses.is_empty()
    {
        let matching = index.statuses_matching(&not_statuses);
        if let Some(cond) = filter_contains(status_field, &matching, true) {
            conditions.push(cond);
        }
    }

    let filter = conditions.join(" AND ");

    Some((text, Some(filter)))
}

fn parse_query(query: &str) -> (Vec<&str>, Vec<&str>, Vec<&str>, String) {
    let mut handles = Vec::new();
    let mut statuses = Vec::new();
    let mut not_statuses = Vec::new();
    let mut parts = Vec::new();
    for part in query.split_whitespace() {
        if let Some(handle) = part.strip_prefix('@')
            && !handle.is_empty()
        {
            handles.push(handle);
        } else if let Some(status) = part.strip_prefix("+")
            && !status.is_empty()
        {
            statuses.push(status);
        } else if let Some(status) = part.strip_prefix("-")
            && !status.is_empty()
        {
            not_statuses.push(status);
        } else {
            parts.push(part);
        }
    }
    (handles, statuses, not_statuses, parts.join(" "))
}

fn filter_contains(field: &str, values: &[&str], not: bool) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let mut f = if not {
        format!("{field} NOT IN [")
    } else {
        format!("{field} IN [")
    };
    for v in values {
        write!(&mut f, "\"{v}\",").expect("fmt write to string never fails");
    }
    f.push(']');
    Some(f)
}

impl Context {
    fn handles_matching(&self, prefixes: &[&str]) -> Vec<&str> {
        self.users
            .iter()
            .filter(|(_, user)| prefixes.iter().any(|h| user.handle_lower.starts_with(*h)))
            .map(|(id, _)| id.as_str())
            .collect()
    }
}

impl meili::Index {
    fn user_field(&self) -> Option<&'static str> {
        match self {
            Self::Diffs => Some("author_id"),
            Self::Tasks => Some("owner_id"),
            Self::Pages => None,
        }
    }

    fn status_field(&self) -> Option<&'static str> {
        match self {
            Self::Diffs => Some("status"),
            Self::Tasks => Some("status"),
            Self::Pages => None,
        }
    }

    fn statuses(&self) -> &'static [&'static str] {
        match self {
            Self::Diffs => Diff::statuses(),
            Self::Tasks => Task::statuses(),
            Self::Pages => Page::statuses(),
        }
    }

    fn statuses_matching(&self, prefixes: &[&str]) -> Vec<&'static str> {
        self.statuses()
            .iter()
            .filter(|s| prefixes.iter().any(|p| s.starts_with(p)))
            .copied()
            .collect()
    }
}

impl meili::Hit {
    fn into_item(self, ctx: &Context) -> Item {
        match self {
            Self::Diff(hit) => hit.into_item(ctx),
            Self::Task(hit) => hit.into_item(ctx),
            Self::Page(hit) => hit.into_item(ctx),
        }
    }
}

impl Diff {
    fn into_item(self, ctx: &Context) -> Item {
        let ago = ago(ctx, self.updated_at);
        let author = ctx
            .users
            .get(&self.author_id)
            .map(|u| u.handle.as_str())
            .unwrap_or("unknown");
        let status = anycase::as_lower(self.status);
        let subtitle = format!("{ago} by {author}, {status}");
        Item::new(format!("D{}: {}", self.id, self.title))
            .subtitle(subtitle)
            .arg(format!("{}/D{}", ctx.config.ph_base_url, self.id))
            .icon(Icon::with_image("./assets/diff.png"))
    }
}

impl Task {
    fn into_item(self, ctx: &Context) -> Item {
        let ago = ago(ctx, self.updated_at);
        let status = anycase::as_lower(self.status);
        let mut subtitle = format!("updated {ago}, {status}");
        if let Some(owner) = self
            .owner_id
            .as_ref()
            .and_then(|id| ctx.users.get(id))
            .map(|user| user.handle.as_str())
        {
            write!(&mut subtitle, ", assigned to {owner}")
                .expect("fmt write to string never fails");
        }
        Item::new(format!("T{}: {}", self.id, self.title))
            .arg(format!("{}/T{}", ctx.config.ph_base_url, self.id))
            .subtitle(subtitle)
            .icon(Icon::with_image("./assets/task.png"))
    }
}

impl Page {
    fn into_item(self, ctx: &Context) -> Item {
        let path = format!("/w/{}", self.path.trim_start_matches('/'));
        Item::new(self.title)
            .arg(format!("{}{}", ctx.config.ph_base_url, path))
            .subtitle(path)
            .icon(Icon::with_image("./assets/wiki.png"))
    }
}

impl Repo {
    fn matches(&self, query: &str) -> bool {
        query
            .split_whitespace()
            .all(|q| self.name.to_lowercase().contains(q))
    }

    fn into_item(self) -> Item {
        let mut item = Item::new(self.name)
            .arg(self.uri)
            .icon(Icon::with_image("./assets/repo.png"));
        if let Some(desc) = self.description {
            item = item.subtitle(desc);
        };
        item
    }
}

fn ago(ctx: &Context, updated_at: i64) -> Cow<'static, str> {
    jiff::Timestamp::from_second(updated_at)
        .ok()
        .map(|t| human::format_ago((ctx.now - t).try_into().unwrap()))
        .unwrap_or_default()
}

fn output(items: impl IntoIterator<Item = Item>) -> Result<()> {
    powerpack::Output::new()
        .items(items)
        .rerun(Duration::from_secs(2))
        .write(io::stdout())?;
    Ok(())
}
