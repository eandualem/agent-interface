//! Team-wide libraries: every skill and every policy an agent can receive,
//! and what each tag brings. Nothing here names an agent in its results; it
//! says how a thing is delivered and how many agents that reaches.
//!
//! Backbone serves these per agent, so a library is gathered from up to
//! [`PREVIEW_LIMIT`] agents' previews and merged by file path.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{Api, Instructions, Member, Skill, Skills, Source, document, instructions, items};
use crate::valid_agent;

/// The most agents whose previews one refresh reads.
pub const PREVIEW_LIMIT: usize = 32;

/// The configured agents, in Backbone's order.
pub(super) fn agents(api: &Api) -> Result<Vec<Member>, &'static str> {
    Ok(items::<Member>(&document(&api.get("/api/agents")?)?)?
        .into_iter()
        .filter(|agent| valid_agent(&agent.name) && agent.configured != Some(false))
        .collect())
}

/// A skill outside the shared store: in a repository, or in a CLI's own
/// skills directory.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct LocalSkill {
    /// Skill name.
    pub name: String,
    /// When to use it.
    pub description: String,
    /// Its folder.
    pub path: String,
    /// Why the skill cannot be loaded, when it cannot.
    pub error: Option<String>,
}

/// The skills one agent's CLI loads at its next launch.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Loadout {
    /// The agent's tags.
    pub tags: Vec<String>,
    /// The shared store's directory.
    pub store: Option<String>,
    /// Store skills linked for it, chosen by their tags.
    pub skills: Vec<Skill>,
    /// Skills in its repository's own skills directories.
    pub repository: Vec<LocalSkill>,
    /// Its CLI's user-level skills directories.
    pub user_directories: Vec<String>,
    /// Skills in those directories.
    pub user: Vec<LocalSkill>,
    /// Backbone's notes about this preview.
    pub notices: Vec<String>,
}

impl Loadout {
    pub(super) fn decode(body: &[u8]) -> Result<Self, &'static str> {
        document(body)?;
        serde_json::from_slice(body).map_err(|_| super::UNREADABLE)
    }
}

/// Why a store skill reaches an agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// It is tagged `all`.
    All,
    /// It carries one of the agent's tags.
    Tag(String),
    /// It names the agent with `agent:NAME`.
    Direct,
}

impl Reason {
    /// The first reason `skill` reaches `agent`, which carries `tags`: `all`,
    /// then a shared tag in the skill's order, then a direct assignment.
    pub fn of(skill_tags: &[String], agent: &str, tags: &[String]) -> Option<Self> {
        if skill_tags.iter().any(|tag| tag == "all") {
            return Some(Self::All);
        }
        if let Some(tag) = skill_tags.iter().find(|tag| tags.contains(tag)) {
            return Some(Self::Tag(tag.clone()));
        }
        skill_tags
            .contains(&format!("agent:{agent}"))
            .then_some(Self::Direct)
    }

    /// How the design words it: `all`, `tag security`, `direct`.
    pub fn label(&self) -> String {
        match self {
            Self::All => "all".into(),
            Self::Tag(tag) => format!("tag {tag}"),
            Self::Direct => "direct".into(),
        }
    }
}

/// Skills kept in one place outside the store, and what that place is.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    /// The repository, or the CLI for a user-level directory.
    pub label: String,
    /// The directory the skills are in.
    pub directory: String,
    /// Its skills, each once.
    pub skills: Vec<LocalSkill>,
}

impl Place {
    fn add(places: &mut Vec<Place>, label: &str, directory: &str, skill: &LocalSkill) {
        let at = match places.iter().position(|place| place.directory == directory) {
            Some(at) => at,
            None => {
                places.push(Place {
                    label: label.to_owned(),
                    directory: directory.to_owned(),
                    skills: Vec::new(),
                });
                places.len() - 1
            }
        };
        let place = &mut places[at];
        if !place.skills.iter().any(|known| known.path == skill.path) {
            place.skills.push(skill.clone());
        }
    }
}

/// Every skill an agent can load, by where it lives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkillLibrary {
    /// The shared store.
    pub store: Skills,
    /// Repository skills, by repository.
    pub repositories: Vec<Place>,
    /// User-level skills, by CLI directory.
    pub user: Vec<Place>,
    /// The configured agents, to answer which of them gets a skill.
    pub agents: Vec<Member>,
    /// How many agents' previews were read; fewer than `agents` past the limit.
    pub previewed: usize,
}

impl SkillLibrary {
    pub(super) fn read(api: &Api) -> Result<Self, &'static str> {
        let store = Skills::decode(&api.get("/api/skills")?)?;
        let agents = agents(api)?;
        let mut library = Self {
            store,
            ..Self::default()
        };
        for agent in agents.iter().take(PREVIEW_LIMIT) {
            // One agent's preview failing leaves the rest of the library.
            let Ok(loadout) = api
                .get(&format!("/api/skills/preview/{}", agent.name))
                .and_then(|body| Loadout::decode(&body))
            else {
                continue;
            };
            library.previewed += 1;
            library.merge(agent, &loadout);
        }
        library.agents = agents;
        Ok(library)
    }

    fn merge(&mut self, agent: &Member, loadout: &Loadout) {
        for skill in &loadout.repository {
            let project = project_of(&skill.path);
            let label = if agent.repo.is_empty() {
                project.clone()
            } else {
                agent.repo.clone()
            };
            Place::add(&mut self.repositories, &label, &project, skill);
        }
        for skill in &loadout.user {
            let directory = loadout
                .user_directories
                .iter()
                .find(|directory| skill.path.starts_with(directory.as_str()))
                .cloned()
                .unwrap_or_else(|| parent(&skill.path));
            Place::add(&mut self.user, &agent.runtime, &directory, skill);
        }
    }
}

fn parent(path: &str) -> String {
    std::path::Path::new(path)
        .parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_default()
}

// A repository skill lives in PROJECT/.claude/skills/NAME or
// PROJECT/.agents/skills/NAME.
fn project_of(path: &str) -> String {
    let path = std::path::Path::new(path);
    path.ancestors()
        .nth(3)
        .map(|project| project.display().to_string())
        .unwrap_or_default()
}

/// The layers a brief is built from, gathered from the agents' briefs: base,
/// the global policies in order, each tag's policies, and swarm briefs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PolicyLibrary {
    /// The base brief.
    pub base: Vec<Source>,
    /// Global policies, in the order every agent receives them.
    pub global: Vec<Source>,
    /// Each tag's policies in order, tags alphabetically.
    pub tags: Vec<(String, Vec<Source>)>,
    /// Swarm briefs, for agents in a swarm.
    pub swarm: Vec<Source>,
    /// How many agents carry each tag.
    pub carriers: BTreeMap<String, usize>,
    /// How many agents' briefs were read.
    pub read: usize,
    /// How many agents there are.
    pub agents: usize,
}

impl PolicyLibrary {
    pub(super) fn read(api: &Api, agents: &[Member]) -> Result<Self, &'static str> {
        let mut briefs = Vec::new();
        for agent in agents.iter().take(PREVIEW_LIMIT) {
            match instructions(api, &agent.name) {
                Ok(brief) => briefs.push(brief),
                Err(super::NOT_SERVED) => return Err(super::NOT_SERVED),
                // One agent's brief failing leaves the rest of the library.
                Err(_) => {}
            }
        }
        Ok(Self::gather(&briefs, agents))
    }

    fn gather(briefs: &[Instructions], agents: &[Member]) -> Self {
        let mut library = Self {
            read: briefs.len(),
            agents: agents.len(),
            ..Self::default()
        };
        for agent in agents {
            for tag in &agent.tags {
                *library.carriers.entry(tag.clone()).or_default() += 1;
            }
        }
        let mut tags: BTreeMap<String, Vec<Source>> = BTreeMap::new();
        // The longest global sequence first, so its order wins.
        let mut ordered: Vec<&Instructions> = briefs.iter().collect();
        ordered.sort_by_key(|brief| std::cmp::Reverse(global_count(brief)));
        for brief in ordered {
            for source in &brief.sources {
                let parts: Vec<&str> = source.scope.split(", ").collect();
                if parts.contains(&"base") {
                    add(&mut library.base, source);
                } else if parts.contains(&"swarm") {
                    add(&mut library.swarm, source);
                } else if parts.contains(&"global") {
                    add(&mut library.global, source);
                } else {
                    for tag in parts.iter().filter_map(|part| part.strip_prefix("tag:")) {
                        add(tags.entry(tag.to_owned()).or_default(), source);
                    }
                }
            }
        }
        library.tags = tags.into_iter().collect();
        library
    }

    /// Every policy once, in layer order: base, global, tags, swarm.
    pub fn all(&self) -> Vec<&Source> {
        let mut all: Vec<&Source> = Vec::new();
        let layers = self
            .base
            .iter()
            .chain(&self.global)
            .chain(self.tags.iter().flat_map(|(_, sources)| sources))
            .chain(&self.swarm);
        for source in layers {
            if !all.iter().any(|known| known.path == source.path) {
                all.push(source);
            }
        }
        all
    }
}

fn global_count(brief: &Instructions) -> usize {
    brief
        .sources
        .iter()
        .filter(|source| source.scope.split(", ").any(|part| part == "global"))
        .count()
}

fn add(sources: &mut Vec<Source>, source: &Source) {
    if !sources.iter().any(|known| known.path == source.path) {
        sources.push(source.clone());
    }
}

/// What each tag brings, from the agents, the store and the policies.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TagLibrary {
    /// The configured agents.
    pub agents: Vec<Member>,
    /// The shared store.
    pub store: Skills,
    /// The policy layers.
    pub policies: PolicyLibrary,
}

/// One tag and what it brings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TagCard {
    /// The tag; `all` is every agent.
    pub tag: String,
    /// How many agents carry it.
    pub carriers: usize,
    /// Store skills tagged with it.
    pub skills: Vec<String>,
    /// Policies it brings, in order.
    pub policies: Vec<String>,
}

impl TagLibrary {
    pub(super) fn read(api: &Api) -> Result<Self, &'static str> {
        let agents = agents(api)?;
        let store = Skills::decode(&api.get("/api/skills")?)?;
        let policies = PolicyLibrary::read(api, &agents)?;
        Ok(Self {
            agents,
            store,
            policies,
        })
    }

    /// One card per tag: `all` first, then every other tag seen on an agent,
    /// a skill or a policy, alphabetically.
    pub fn cards(&self) -> Vec<TagCard> {
        let skills = |tag: &str| -> Vec<String> {
            self.store
                .items
                .iter()
                .filter(|skill| skill.tags.iter().any(|t| t == tag))
                .map(|skill| skill.name.clone())
                .collect()
        };
        let mut cards = vec![TagCard {
            tag: "all".into(),
            carriers: self.agents.len(),
            skills: skills("all"),
            policies: self
                .policies
                .base
                .iter()
                .chain(&self.policies.global)
                .map(|source| source.name.clone())
                .collect(),
        }];
        let mut names: Vec<&str> = self
            .agents
            .iter()
            .flat_map(|agent| &agent.tags)
            .chain(self.store.items.iter().flat_map(|skill| &skill.tags))
            .map(String::as_str)
            .chain(self.policies.tags.iter().map(|(tag, _)| tag.as_str()))
            .filter(|tag| *tag != "all" && !tag.starts_with("agent:"))
            .collect();
        names.sort_unstable();
        names.dedup();
        for tag in names {
            cards.push(TagCard {
                tag: tag.to_owned(),
                carriers: self.policies.carriers.get(tag).copied().unwrap_or_else(|| {
                    self.agents
                        .iter()
                        .filter(|agent| agent.tags.iter().any(|t| t == tag))
                        .count()
                }),
                skills: skills(tag),
                policies: self
                    .policies
                    .tags
                    .iter()
                    .find(|(name, _)| name == tag)
                    .map(|(_, sources)| sources.iter().map(|s| s.name.clone()).collect())
                    .unwrap_or_default(),
            });
        }
        cards
    }

    /// Store skills given directly to agents by name, with how many agents.
    pub fn direct(&self) -> Vec<(String, usize)> {
        self.store
            .items
            .iter()
            .map(|skill| {
                let count = skill
                    .tags
                    .iter()
                    .filter(|tag| tag.starts_with("agent:"))
                    .count();
                (skill.name.clone(), count)
            })
            .filter(|(_, count)| *count > 0)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(name: &str, scope: &str) -> Source {
        Source {
            name: name.into(),
            path: format!("/t/{name}.md"),
            scope: scope.into(),
            applied: true,
            text: Some(format!("# {name}")),
        }
    }

    fn agent(name: &str, tags: &[&str]) -> Member {
        Member {
            name: name.into(),
            tags: tags.iter().map(|tag| (*tag).into()).collect(),
            ..Member::default()
        }
    }

    #[test]
    fn a_store_skill_reaches_an_agent_by_all_then_tag_then_name() {
        let tags = |list: &[&str]| -> Vec<String> { list.iter().map(|t| (*t).into()).collect() };
        let mine = tags(&["coder", "security"]);
        assert_eq!(
            Reason::of(&tags(&["all", "coder"]), "worker", &mine),
            Some(Reason::All)
        );
        assert_eq!(
            Reason::of(&tags(&["presence", "security"]), "worker", &mine),
            Some(Reason::Tag("security".into()))
        );
        assert_eq!(
            Reason::of(&tags(&["agent:worker"]), "worker", &mine),
            Some(Reason::Direct)
        );
        assert_eq!(Reason::of(&tags(&["agent:other"]), "worker", &mine), None);
        assert_eq!(Reason::Tag("security".into()).label(), "tag security");
    }

    #[test]
    fn policy_layers_merge_briefs_in_order_without_naming_agents() {
        let coder = Instructions {
            name: "worker".into(),
            sources: vec![
                source("base", "base"),
                source("collaboration", "global"),
                source("continuity", "global"),
                source("minimal-change", "tag:coder"),
                source("review", "tag:coder, tag:security"),
            ],
            ..Instructions::default()
        };
        let member = Instructions {
            name: "helper".into(),
            sources: vec![
                source("base", "base"),
                source("collaboration", "global"),
                source("swarm", "swarm"),
            ],
            ..Instructions::default()
        };
        let agents = [
            agent("worker", &["coder", "security"]),
            agent("helper", &[]),
        ];
        let library = PolicyLibrary::gather(&[member, coder], &agents);
        let names = |sources: &[Source]| -> Vec<String> {
            sources.iter().map(|source| source.name.clone()).collect()
        };
        assert_eq!(names(&library.base), ["base"]);
        assert_eq!(names(&library.global), ["collaboration", "continuity"]);
        assert_eq!(names(&library.swarm), ["swarm"]);
        assert_eq!(library.tags.len(), 2);
        assert_eq!(library.tags[0].0, "coder");
        assert_eq!(names(&library.tags[0].1), ["minimal-change", "review"]);
        assert_eq!(names(&library.tags[1].1), ["review"]);
        assert_eq!(library.carriers.get("coder"), Some(&1));
        assert_eq!(library.all().len(), 6);
    }

    #[test]
    fn tag_cards_put_all_first_and_count_direct_skills_once() {
        let skill = |name: &str, tags: &[&str]| Skill {
            name: name.into(),
            tags: tags.iter().map(|tag| (*tag).into()).collect(),
            ..Skill::default()
        };
        let library = TagLibrary {
            agents: vec![agent("worker", &["coder"]), agent("helper", &["coder"])],
            store: Skills {
                store: None,
                items: vec![
                    skill("check", &["all"]),
                    skill("audit", &["security", "agent:worker", "agent:helper"]),
                    skill("layout", &["xstate"]),
                ],
            },
            policies: PolicyLibrary {
                base: vec![source("base", "base")],
                global: vec![source("collaboration", "global")],
                tags: vec![("review".into(), vec![source("review", "tag:review")])],
                ..PolicyLibrary::default()
            },
        };
        let cards = library.cards();
        let tags: Vec<&str> = cards.iter().map(|card| card.tag.as_str()).collect();
        assert_eq!(tags, ["all", "coder", "review", "security", "xstate"]);
        assert_eq!(cards[0].carriers, 2);
        assert_eq!(cards[0].skills, ["check"]);
        assert_eq!(cards[0].policies, ["base", "collaboration"]);
        assert_eq!(cards[1].carriers, 2);
        assert_eq!(cards[2].policies, ["review"]);
        assert_eq!(cards[4].carriers, 0);
        assert_eq!(library.direct(), [("audit".to_owned(), 2)]);
    }

    #[test]
    fn previews_merge_repository_and_user_skills_by_place() {
        let loadout = Loadout::decode(
            br#"{"name":"worker","runtime":"claude","tags":["coder"],"store":"/s",
            "skills":[{"name":"check","tags":["all"],"description":"d","path":"/s/check"}],
            "repository":[{"name":"local","description":"l","error":null,
              "path":"/w/app/.claude/skills/local","directories":[".claude/skills"]}],
            "user_directories":["/h/.claude/skills"],
            "user":[{"name":"mine","description":"m","error":null,
              "path":"/h/.claude/skills/mine","directory":"/h/.claude/skills"}],
            "notices":["plugins are not listed"]}"#,
        )
        .unwrap();
        assert_eq!(loadout.skills[0].tags, ["all"]);
        assert_eq!(loadout.notices, ["plugins are not listed"]);
        let mut library = SkillLibrary::default();
        let worker = Member {
            repo: "example/app".into(),
            runtime: "claude".into(),
            ..agent("worker", &["coder"])
        };
        library.merge(&worker, &loadout);
        library.merge(&worker, &loadout);
        assert_eq!(library.repositories.len(), 1);
        assert_eq!(library.repositories[0].label, "example/app");
        assert_eq!(library.repositories[0].directory, "/w/app");
        assert_eq!(library.repositories[0].skills.len(), 1);
        assert_eq!(library.user[0].label, "claude");
        assert_eq!(library.user[0].directory, "/h/.claude/skills");
        assert!(Loadout::decode(b"[]").is_err());
    }
}
