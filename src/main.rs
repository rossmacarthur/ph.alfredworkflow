mod config;
mod human;
mod ph;

use std::collections::HashMap;
use std::env;
use std::fmt::Write as _;
use std::io;
use std::time::Duration;

use anyhow::Result;
use constcat::concat;
use powerpack::Icon;
use powerpack::Item;
use powerpack::logger;

use crate::config::Config;
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
    Repos,
    Diffs,
    Tasks,
    Wiki,
}

fn main() -> Result<()> {
    if let Err(err) = run() {
        eprintln!("{err:#}");
        let item = Item::new(format!("Error: {err}")).subtitle(
            "The workflow errored! \
             You might want to try debugging it or checking the logs.",
        );
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

    let cmds = [
        Command::Repos,
        Command::Diffs,
        Command::Tasks,
        Command::Wiki,
    ];

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
                        let item = Item::new("No command found");
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
    fn name(&self) -> &'static str {
        match self {
            Command::Repos => "repos",
            Command::Diffs => "diffs",
            Command::Tasks => "tasks",
            Command::Wiki => "wiki",
        }
    }

    fn subtitle(&self) -> &'static str {
        match self {
            Command::Repos => "Search repositories",
            Command::Diffs => "Search active revisions",
            Command::Tasks => "Search maniphest tasks",
            Command::Wiki => "Search wiki pages",
        }
    }

    fn icon(&self) -> &'static str {
        match self {
            Command::Repos => "repo.png",
            Command::Diffs => "diff.png",
            Command::Tasks => "task.png",
            Command::Wiki => "wiki.png",
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
            Command::Repos => ph::repos(&ctx.config)?
                .into_iter()
                .filter(|r| r.matches(query))
                .map(|r| r.into_item())
                .collect(),
            Command::Diffs => ph::diffs(&ctx.config)?
                .into_iter()
                .filter(|d| d.matches(ctx, query))
                .map(|d| d.into_item(ctx))
                .collect(),
            Command::Tasks => ph::tasks(&ctx.config)?
                .into_iter()
                .filter(|t| t.matches(ctx, query))
                .map(|t| t.into_item(ctx))
                .collect(),
            Command::Wiki => ph::documents(&ctx.config)?
                .into_iter()
                .filter(|d| d.matches(query))
                .map(|d| d.into_item(ctx))
                .collect(),
        };
        Ok(items)
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
            .icon(Icon::with_image("repo.png"));
        if let Some(desc) = self.description {
            item = item.subtitle(desc);
        };
        item
    }
}

impl Diff {
    fn matches(&self, ctx: &Context, query: &str) -> bool {
        query.split_whitespace().all(|q| {
            if let Some(q) = q.strip_prefix('@') {
                ctx.users
                    .get(&self.author_phid)
                    .is_some_and(|user| user.matches(q))
            } else {
                self.id_title.to_lowercase().contains(q)
            }
        })
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
        Item::new(self.id_title)
            .subtitle(subtitle)
            .arg(self.uri)
            .icon(Icon::with_image("diff.png"))
    }
}

impl Task {
    fn matches(&self, ctx: &Context, query: &str) -> bool {
        query.split_whitespace().all(|q| {
            if let Some(q) = q.strip_prefix('@') {
                self.owner_phid
                    .as_ref()
                    .and_then(|phid| ctx.users.get(phid))
                    .is_some_and(|user| user.matches(q))
            } else {
                self.id_title.to_lowercase().contains(q)
            }
        })
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
            write!(&mut subtitle, ", assigned to {owner}").expect("fmt write never fails");
        }
        Item::new(self.id_title)
            .arg(self.uri)
            .subtitle(subtitle)
            .icon(Icon::with_image("task.png"))
    }
}

impl Document {
    fn matches(&self, query: &str) -> bool {
        query.split_whitespace().all(|q| {
            self.title.to_lowercase().contains(q)
                || self.path.contains(q)
                || self.content.to_lowercase().contains(q)
        })
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
            .icon(Icon::with_image("wiki.png"))
    }
}

impl User {
    fn matches(&self, query: &str) -> bool {
        self.handle.to_lowercase().contains(query) || self.real_name.to_lowercase().contains(query)
    }
}

fn output(items: impl IntoIterator<Item = Item>) -> Result<()> {
    powerpack::Output::new()
        .items(items)
        .rerun(Duration::from_secs(2))
        .write(io::stdout())?;
    Ok(())
}
