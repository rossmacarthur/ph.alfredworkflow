use serde::Deserialize;
use serde::Serialize;
use serde_json as json;
use serde_json::json;

/// A user
#[derive(Debug, Clone)]
pub struct User {
    pub id: String,
    pub handle: String,
    pub handle_lower: String,
}

/// A repository
#[derive(Debug, Clone)]
pub struct Repo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub uri: String,
    pub monogram: String,
    pub monogram_lower: String,
}

/// A diff
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Diff {
    pub id: u32,
    pub title: String,
    pub author_id: String,
    pub status: String,
    pub repository_id: Option<String>,
    pub updated_at: i64,
}

/// A task
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub description: String,
    pub owner_id: Option<String>,
    pub status: String,
    pub updated_at: i64,
}

/// A wiki page
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Page {
    pub id: u32,
    pub path: String,
    pub status: String,
    pub title: String,
    pub content: String,
}

impl Diff {
    pub fn statuses() -> &'static [&'static str] {
        &[
            "needs-review",
            "needs-revision",
            "changes-planned",
            "accepted",
            "published",
            "abandoned",
        ]
    }

    pub fn rank(&self) -> u32 {
        match &*self.status {
            "needs-review" => 1,
            "needs-revision" => 2,
            "changes-planned" => 3,
            "accepted" => 4,
            "published" => 5,
            "abandoned" => 100,
            _ => 50, // unknown statuses get medium priority
        }
    }

    pub fn settings() -> json::Value {
        json!({
            "searchableAttributes": ["title"],
            "filterableAttributes": ["author_id", "status", "repository_id"],
            "rankingRules": ["rank:asc", "updated_at:desc", "words", "typo", "proximity", "attribute", "sort", "exactness"],
            "sortableAttributes": ["updated_at", "rank"],
        })
    }
}

impl Task {
    pub fn statuses() -> &'static [&'static str] {
        &[
            "open",
            "resolved",
            "wontfix",
            "invalid",
            "duplicate",
            "spite",
        ]
    }

    pub fn rank(&self) -> u32 {
        match &*self.status {
            "open" => 1,
            "resolved" => 100,
            "wontfix" => 101,
            "invalid" => 102,
            "duplicate" => 103,
            "spite" => 104,
            _ => 50, // unknown statuses get medium priority
        }
    }

    pub fn settings() -> json::Value {
        json!({
            "searchableAttributes": ["title", "description"],
            "filterableAttributes": ["owner_id", "status"],
            "rankingRules": ["rank:asc", "updated_at:desc", "words", "typo", "proximity", "attribute", "sort", "exactness"],
            "sortableAttributes": ["updated_at", "rank"],
        })
    }
}

impl Page {
    pub fn statuses() -> &'static [&'static str] {
        &["active", "moved", "deleted"]
    }

    pub fn rank(&self) -> u32 {
        match &*self.status {
            "active" => 1,
            "moved" => 100,
            "deleted" => 101,
            _ => 50, // unknown statuses get medium priority
        }
    }

    pub fn settings() -> json::Value {
        json!({
            "searchableAttributes": ["title", "path", "content"],
            "filterableAttributes": ["status"],
            "rankingRules": ["rank:asc", "words", "typo", "proximity", "attribute", "sort", "exactness"],
            "sortableAttributes": ["rank"],
        })
    }
}
