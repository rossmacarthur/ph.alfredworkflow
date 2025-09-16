mod config;
mod human;
mod ord_float;
mod ph;

use std::cmp::Reverse;
use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::io;
use std::time::Duration;

use anyhow::Result;
use constcat::concat;
use itermore::IterSorted as _;
use powerpack::Icon;
use powerpack::Item;
use powerpack::cache;
use powerpack::logger;
use then::Some as _;

use crate::config::Config;
use crate::ord_float::OrdFloat;
use crate::ph::Diff;
use crate::ph::Document;
use crate::ph::Repo;
use crate::ph::Task;
use crate::ph::User;

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
    Wiki,
    Repos,
}

fn main() -> Result<()> {
    if let Err(err) = run() {
        eprintln!("ERROR: {err:#}");
        let item = if let Some(cache::QueryError::Miss) = err.downcast_ref::<cache::QueryError>() {
            Item::new(format!("Warning: {err}"))
                .subtitle("The workflow is still loading data from Phabricator/Phorge")
                .icon(Icon::with_image("./assets/warning.png"))
        } else {
            Item::new(format!("Error: {err}"))
                .subtitle(
                    "The workflow errored! \
                     You might want to try debugging it or checking the logs",
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
        .map(|u| (u.phid.clone(), u))
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

trait CmpKey: Ord + Clone + Copy + Sized {}
impl<T> CmpKey for T where T: Ord + Clone + Copy + Sized {}

impl Command {
    fn all() -> [Self; 4] {
        [Self::Diffs, Self::Tasks, Self::Wiki, Self::Repos]
    }

    fn name(&self) -> &'static str {
        match self {
            Self::Diffs => "diffs",
            Self::Tasks => "tasks",
            Self::Wiki => "wiki",
            Self::Repos => "repos",
        }
    }

    fn subtitle(&self) -> &'static str {
        match self {
            Self::Diffs => "Search active revisions",
            Self::Tasks => "Search maniphest tasks",
            Self::Wiki => "Search wiki pages",
            Self::Repos => "Search repositories",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Self::Diffs => "./assets/diff.png",
            Self::Tasks => "./assets/task.png",
            Self::Wiki => "./assets/wiki.png",
            Self::Repos => "./assets/repo.png",
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
        let items = match self {
            Self::Diffs => ph::diffs(&ctx.config)?
                .into_iter()
                .filter_map(|d| {
                    let (ok, cmp) = d.filter_cmp_key(ctx, query);
                    ok.some((d, cmp))
                })
                .sorted_by_key(|&(_, cmp)| cmp)
                .map(|(d, _)| d.into_item(ctx))
                .take(20)
                .collect(),

            Self::Tasks => ph::tasks(&ctx.config)?
                .into_iter()
                .filter_map(|t| {
                    let (ok, cmp) = t.filter_cmp_key(ctx, query);
                    ok.some((t, cmp))
                })
                .sorted_by_key(|&(_, cmp)| cmp)
                .map(|(t, _)| t.into_item(ctx))
                .take(20)
                .collect(),

            Self::Wiki => ph::documents(&ctx.config)?
                .into_iter()
                .filter(|d| d.matches(query))
                .sorted_by_key(|d| d.cmp_key(query))
                .map(|d| d.into_item(ctx))
                .take(20)
                .collect(),

            Self::Repos => ph::repos(&ctx.config)?
                .into_iter()
                .filter(|r| r.matches(query))
                .map(|r| r.into_item())
                .take(20)
                .collect(),
        };
        Ok(items)
    }
}

impl Diff {
    fn filter_cmp_key(&self, ctx: &Context, query: &str) -> (bool, impl CmpKey + use<>) {
        let (handles, rest): (Vec<_>, Vec<_>) =
            query.split_whitespace().partition(|q| q.starts_with('@'));

        let filter_owner = handles.into_iter().all(|h| {
            h.strip_prefix('@')
                .map(|u| {
                    ctx.users
                        .get(&self.author_phid)
                        .is_some_and(|user| user.matches(u))
                })
                .unwrap_or(false)
        });

        let filter =
            query.is_empty() || (filter_owner && rest.iter().all(|q| self.title_lower.contains(q)));

        let query = rest.join(" ");

        let cmp = (
            Reverse(filter_owner),
            Reverse(self.title_lower.starts_with(&query)),
            Reverse(self.title_lower.contains(&query)),
            Reverse(self.updated),
        );

        (filter, cmp)
    }

    fn into_item(self, ctx: &Context) -> Item {
        let ago = human::format_ago((ctx.now - self.updated).try_into().unwrap());
        let author = ctx
            .users
            .get(&self.author_phid)
            .map(|u| u.handle.as_str())
            .unwrap_or("unknown");
        let status = anycase::as_lower(self.status);
        let subtitle = format!("{ago} by {author}, {status}");
        Item::new(format!("D{}: {}", self.id, self.title))
            .subtitle(subtitle)
            .arg(self.uri)
            .icon(Icon::with_image("./assets/diff.png"))
    }
}

impl Task {
    fn filter_cmp_key(&self, ctx: &Context, query: &str) -> (bool, impl CmpKey + use<>) {
        let (handles, rest): (Vec<_>, Vec<_>) =
            query.split_whitespace().partition(|q| q.starts_with('@'));

        let filter_owner = handles.into_iter().all(|h| {
            h.strip_prefix('@')
                .map(|u| {
                    self.owner_phid
                        .as_ref()
                        .and_then(|phid| ctx.users.get(phid))
                        .is_some_and(|user| user.matches(u))
                })
                .unwrap_or(false)
        });

        let filter = query.is_empty()
            || (filter_owner
                && rest
                    .iter()
                    .all(|q| self.title_lower.contains(q) || self.description_lower.contains(q)));

        let query = rest.join(" ");

        let cmp = (
            Reverse(filter_owner),
            Reverse(self.title_lower.starts_with(&query)),
            Reverse(self.title_lower.contains(&query)),
            Reverse(self.description_lower.contains(&query)),
            Reverse(OrdFloat({
                let score = strsim::jaro_winkler(&self.title_lower, &query);
                (score * 10.0).round() / 10.0
            })),
            Reverse(self.updated),
        );

        (filter, cmp)
    }

    fn into_item(self, ctx: &Context) -> Item {
        let ago = human::format_ago((ctx.now - self.updated).try_into().unwrap());
        let mut subtitle = format!("updated {ago}");
        if let Some(owner) = self
            .owner_phid
            .as_ref()
            .and_then(|phid| ctx.users.get(phid))
            .map(|user| user.handle.as_str())
        {
            write!(&mut subtitle, ", assigned to {owner}")
                .expect("fmt write to string never fails");
        }
        Item::new(format!("T{}: {}", self.id, self.title))
            .arg(self.uri)
            .subtitle(subtitle)
            .icon(Icon::with_image("./assets/task.png"))
    }
}

impl Document {
    fn matches(&self, query: &str) -> bool {
        query.is_empty()
            || query.split_whitespace().any(|q| {
                self.path_lower.contains(q)
                    || self.title_lower.contains(q)
                    || self.content_lower.contains(q)
            })
    }

    fn cmp_key(&self, query: &str) -> impl CmpKey + use<> {
        (
            Reverse(self.path_lower.starts_with(query)),
            Reverse(self.title_lower.starts_with(query)),
            Reverse(self.path_lower.contains(query)),
            Reverse(self.title_lower.contains(query)),
            Reverse(self.content_lower.contains(query)),
            Reverse(OrdFloat({
                let score = strsim::jaro_winkler(&self.title_lower, query);
                (score * 10.0).round() / 10.0
            })),
            Reverse(self.id),
        )
    }

    fn into_item(self, ctx: &Context) -> Item {
        let path = format!("/w/{}", self.path.trim_start_matches('/'));
        Item::new(self.title)
            .arg(format!(
                "{}{}",
                ctx.config.api_url.trim_end_matches("/api/"),
                path,
            ))
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

impl User {
    fn matches(&self, query: &str) -> bool {
        self.handle_lower.starts_with(query)
    }
}

fn output(items: impl IntoIterator<Item = Item>) -> Result<()> {
    powerpack::Output::new()
        .items(items)
        .rerun(Duration::from_secs(2))
        .write(io::stdout())?;
    Ok(())
}
