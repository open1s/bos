//! Authoring skills from the GUI — the round trip from a form to an
//! instruction the agent actually reads.
//!
//! A skill in BOS is a directory holding a `SKILL.md`: YAML frontmatter that
//! names and describes it, and a body that is the instruction the model reads.
//! [`agent::skills::SkillLoader`] is the only reader, so this module has one
//! job — produce exactly the file that loader parses, and prove it by reading it
//! back through the loader rather than by trusting the writer. The name and
//! description rules are the agent's own
//! ([`agent::skills::SkillAuthoringRecord::validate`]); the GUI adds only the
//! two things the loader cannot supply: a body that is non-empty and bounded,
//! and a name that cannot walk out of the skills directory.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Longest instructions body this module will write.
///
/// The loader reads the whole file into the agent's prompt surface, so a skill
/// is an instruction and not a corpus; past this the right shape is a knowledge
/// entry the agent queries on demand.
pub(crate) const MAX_BODY_CHARS: usize = 12_000;

/// A skill as the pane lists it: what the agent would receive, where it came
/// from, and whether this workspace has switched it off.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Skill {
    /// The name the frontmatter declares, which is how the loader keys it.
    pub(crate) name: String,
    /// One line, from the frontmatter.
    pub(crate) description: String,
    /// The `SKILL.md` this came from.
    pub(crate) path: String,
    /// Characters of instructions — the string the model gets, not the file
    /// size, so the pane counts what is actually spent.
    pub(crate) instructions_chars: usize,
    /// True when the workspace's deny-list hides it from this workspace.
    pub(crate) denied: bool,
}

/// The pane's answer: where skills are read from, what is there, and the bounds
/// in force. The same shape answers a list and a save, so the pane redraws from
/// one function.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Report {
    /// The caller's clock, echoed back.
    pub(crate) now_ms: u64,
    /// The directory that was read, after tilde expansion.
    pub(crate) dir: String,
    /// False when the directory does not exist yet — an empty skills directory
    /// discovers nothing, which is not an error.
    pub(crate) exists: bool,
    /// The discovered skills, by name.
    pub(crate) skills: Vec<Skill>,
    /// Bounds in force.
    pub(crate) max_body_chars: usize,
    /// The agent's own limit, reported so the form can say it.
    pub(crate) max_name_chars: usize,
    /// The agent's own limit, reported so the form can say it.
    pub(crate) max_description_chars: usize,
}

/// What a save wrote.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Saved {
    /// The name as validated (trimmed).
    pub(crate) name: String,
    /// The `SKILL.md` that now exists.
    pub(crate) path: String,
    /// True when a file was already there and `overwrite` allowed replacing it.
    pub(crate) replaced: bool,
    /// Characters of instructions written.
    pub(crate) instructions_chars: usize,
}

/// The single path segment a skill name may map to, or `None` when the name
/// would resolve outside the skills directory.
///
/// The name is used verbatim (a lossy rewrite could make two skills share a
/// directory), so the check is what keeps this honest: no separator, and
/// neither `.` nor `..`. Everything else — dots, dashes, non-ASCII — is a
/// legitimate directory name.
pub(crate) fn slug(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        return None;
    }
    Some(name.to_string())
}

/// Validate a skill before it is written, returning every problem found.
///
/// The name and description rules are the agent's, so a skill the GUI accepts
/// is one the loader's own authoring record accepts; the body and the path are
/// this module's, because the loader reads a body without checking it (an empty
/// skill would be a name with nothing to follow) and writes a path without a
/// containment rule.
pub(crate) fn validate(name: &str, description: &str, body: &str) -> Result<(), Vec<String>> {
    let record = agent::skills::SkillAuthoringRecord::new(
        name.trim(),
        description.trim(),
        serde_json::json!({}),
    );
    let mut errors: Vec<String> = record
        .validate()
        .err()
        .unwrap_or_default()
        .iter()
        .map(|error| error.to_string())
        .collect();

    let body = body.trim();
    if body.is_empty() {
        errors.push("body: must not be empty".to_string());
    } else if body.chars().count() > MAX_BODY_CHARS {
        errors.push(format!("body: must be at most {MAX_BODY_CHARS} characters"));
    }

    if slug(name).is_none() {
        errors.push("name: must be one path segment (no / or \\, and not . or ..)".to_string());
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// The `SKILL.md` text for a validated skill.
///
/// Both scalars are emitted as JSON strings, which are valid YAML
/// double-quoted scalars: a description containing `:`, `#`, quotes or a
/// leading `-` then survives the round trip instead of splitting the document.
fn render(name: &str, description: &str, body: &str) -> String {
    let quoted = |value: &str| serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        quoted(name),
        quoted(description),
        body
    )
}

/// Write `text` to `path` through a temporary file in the same directory, so a
/// reader never sees a half-written skill.
fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    std::fs::create_dir_all(dir)
        .map_err(|error| format!("could not create {}: {error}", dir.display()))?;
    let tmp = dir.join("SKILL.md.tmp");
    std::fs::write(&tmp, text)
        .map_err(|error| format!("could not write {}: {error}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not move {} into place: {error}", tmp.display())
    })
}

/// Validate and write a skill. Refuses to replace an existing skill unless
/// `overwrite` is set, and names the file it would replace when it refuses.
pub(crate) fn save(
    dir: &Path,
    name: &str,
    description: &str,
    body: &str,
    overwrite: bool,
) -> Result<Saved, String> {
    let (name, description, body) = (name.trim(), description.trim(), body.trim());
    if let Err(errors) = validate(name, description, body) {
        return Err(errors.join("; "));
    }
    let slug = slug(name).expect("validate accepted the name");
    let path = dir.join(&slug).join("SKILL.md");
    let replaced = path.exists();
    if replaced && !overwrite {
        return Err(format!(
            "{} already exists — replace it to overwrite",
            path.display()
        ));
    }
    write_atomic(&path, &render(name, description, body))?;
    Ok(Saved {
        name: name.to_string(),
        path: path.display().to_string(),
        replaced,
        instructions_chars: body.chars().count(),
    })
}

/// Discover the skills in `dir`, as the agent would see them.
///
/// Reading through [`agent::skills::SkillLoader`] is the point: the pane's
/// count and the agent's prompt are then the same number by construction, and a
/// file the loader cannot parse is absent from both rather than listed and
/// silently dropped at run time.
pub(crate) fn list(dir: &Path, deny: &[String]) -> (bool, Vec<Skill>) {
    let exists = dir.is_dir();
    let mut loader = agent::skills::SkillLoader::new(dir.to_path_buf());
    let discovered = loader.discover().unwrap_or_default();
    let mut skills: Vec<Skill> = discovered
        .iter()
        .filter_map(|meta| {
            let content = loader.load(&meta.name)?;
            Some(Skill {
                name: meta.name.clone(),
                description: meta.description.clone(),
                path: meta.path.display().to_string(),
                instructions_chars: content.instructions.chars().count(),
                denied: deny.iter().any(|name| name == &meta.name),
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    (exists, skills)
}

/// The pane's report for a directory and a workspace deny-list.
pub(crate) fn report(dir: &Path, deny: &[String], now_ms: u64) -> Report {
    let (exists, skills) = list(dir, deny);
    Report {
        now_ms,
        dir: dir.display().to_string(),
        exists,
        skills,
        max_body_chars: MAX_BODY_CHARS,
        max_name_chars: agent::skills::SkillAuthoringRecord::MAX_NAME_LEN,
        max_description_chars: agent::skills::SkillAuthoringRecord::MAX_DESCRIPTION_LEN,
    }
}

// ---------------------------------------------------------------------------
// Which chat carries which skill
// ---------------------------------------------------------------------------

/// Most names one chat's record may carry back from disk.
///
/// This is not a cap on the feature — a record is written from the skills the
/// workspace actually offers, so a directory of 300 skills is a chat that may
/// switch on 300. It is the bound a *hand-edited* file has to fit, because the
/// list ends up in the agent's registration call.
const MAX_NAMES_PER_RECORD: usize = 256;

/// Most chats the activation file will remember.
///
/// A record is per chat and permanent until it is cleared, so this is a bound
/// on a file that otherwise grows with every chat ever opened.
pub(crate) const MAX_SESSIONS: usize = 500;

/// The shape version of `skill-activation.json`.
const ACTIVATION_VERSION: u32 = 1;

/// Which skills are switched on in which chat.
///
/// A chat with **no record inherits the workspace** — every skill the directory
/// holds, minus the workspace's own denials — while a chat *with* a record
/// receives exactly the skills that record names. The asymmetry is the feature:
/// switching one skill on for a chat is a statement about that chat's job, so
/// the other skills stop being carried there; and a chat nobody has touched
/// keeps following the workspace, which is what makes a newly authored skill
/// visible everywhere instead of nowhere.
///
/// The record is an allow-list, deliberately the opposite reading from the
/// workspace's deny-list: the workspace answers "what could a chat use", the
/// chat answers "what is this chat's job". A workspace denial stays a **floor**
/// either way — a chat cannot switch on what its workspace switched off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Activation {
    /// Shape version. A file written by another shape is not read as this one,
    /// and the field's default is `0` (not the current version) so a file that
    /// predates the stamp cannot look valid.
    #[serde(default)]
    version: u32,
    /// Session id to the skills that chat switched on, sorted.
    #[serde(default)]
    sessions: BTreeMap<String, Vec<String>>,
}

impl Default for Activation {
    fn default() -> Self {
        Self {
            version: ACTIVATION_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

impl Activation {
    /// The store the app opens: `~/.bos/gui/skill-activation.json`.
    pub(crate) fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".bos")
            .join("gui")
            .join("skill-activation.json")
    }

    /// Open the default store, tolerating anything a file can be.
    pub(crate) fn open() -> Self {
        Self::load(&Self::default_path())
    }

    /// Load `path`. A missing file, unreadable text, another shape's file or a
    /// hand-edited record all read as "this chat inherits": losing an
    /// activation is a smaller failure than failing to open a chat, and the
    /// next write stamps the current shape.
    pub(crate) fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(file) = serde_json::from_str::<Activation>(&text) else {
            return Self::default();
        };
        if file.version != ACTIVATION_VERSION {
            return Self::default();
        }
        file.sanitized()
    }

    /// Drop what cannot be used: blank ids and names, duplicates, and anything
    /// past the caps. A hand-edited file must not be able to make a command
    /// fail, and a name that is not sorted twice must not draw twice.
    fn sanitized(mut self) -> Self {
        let mut kept: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (session, names) in std::mem::take(&mut self.sessions) {
            let session = session.trim().to_string();
            if session.is_empty() {
                continue;
            }
            let mut unique: Vec<String> = Vec::new();
            for name in names {
                let name = name.trim();
                if name.is_empty() || unique.iter().any(|seen| seen == name) {
                    continue;
                }
                unique.push(name.to_string());
                if unique.len() == MAX_NAMES_PER_RECORD {
                    break;
                }
            }
            unique.sort();
            // An empty list is a real state ("this chat carries nothing"), so it
            // is kept; only a blank session id is dropped.
            if kept.len() == MAX_SESSIONS && !kept.contains_key(&session) {
                continue;
            }
            kept.insert(session, unique);
        }
        self.sessions = kept;
        self
    }

    /// Write the file atomically, so a reader never sees half a record.
    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("could not create {}: {err}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(&self)
            .map_err(|err| format!("could not encode the activation record: {err}"))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .map_err(|err| format!("could not write {}: {err}", tmp.display()))?;
        std::fs::rename(&tmp, path)
            .map_err(|err| format!("could not replace {}: {err}", path.display()))
    }

    /// The skills `session` switched on, or `None` when it inherits.
    pub(crate) fn active(&self, session: &str) -> Option<&[String]> {
        self.sessions.get(session).map(Vec::as_slice)
    }

    /// Whether `session` has a record of its own.
    pub(crate) fn is_explicit(&self, session: &str) -> bool {
        self.sessions.contains_key(session)
    }

    /// How many chats have a record.
    pub(crate) fn record_count(&self) -> usize {
        self.sessions.len()
    }

    /// Switch `name` on or off for `session`, returning whether anything changed.
    ///
    /// A chat with no record materializes what it inherits from `available`
    /// first, so switching one skill *off* in a chat that was following the
    /// workspace leaves the chat carrying the others rather than only none.
    pub(crate) fn set(
        &mut self,
        session: &str,
        name: &str,
        on: bool,
        available: &[String],
    ) -> Result<bool, String> {
        let session = session.trim();
        if session.is_empty() {
            return Err("a chat is needed to switch a skill on or off".to_string());
        }
        if on && !available.iter().any(|held| held == name) {
            return Err(format!(
                "this workspace does not offer a skill named {name:?}"
            ));
        }
        if !self.sessions.contains_key(session) && self.sessions.len() >= MAX_SESSIONS {
            return Err(format!(
                "the activation file already remembers {MAX_SESSIONS} chats; clear one first"
            ));
        }
        let mut list = self
            .sessions
            .get(session)
            .cloned()
            .unwrap_or_else(|| available.to_vec());
        let held = list.iter().any(|held| held == name);
        match (on, held) {
            // No cap on the push: a record can only ever hold names the
            // workspace offers, because switching on a name it does not offer is
            // refused above. A constant here would refuse a workspace with more
            // skills than the constant.
            (true, false) => list.push(name.to_string()),
            (false, true) => list.retain(|held| held != name),
            _ => return Ok(false),
        }
        list.sort();
        self.sessions.insert(session.to_string(), list);
        Ok(true)
    }

    /// Drop the record: the chat inherits the workspace again. Returns whether
    /// there was one to drop.
    pub(crate) fn clear(&mut self, session: &str) -> bool {
        self.sessions.remove(session.trim()).is_some()
    }
}

/// What a chat receives: the workspace's skills, narrowed to the chat's record
/// when it has one, with a workspace denial as a floor in both cases.
pub(crate) fn effective(
    discovered: &[String],
    denied: &[String],
    active: Option<&[String]>,
) -> Vec<String> {
    let denied: HashSet<&String> = denied.iter().collect();
    let allowed = discovered
        .iter()
        .filter(|name| !denied.contains(name))
        .collect::<Vec<_>>();
    match active {
        None => allowed.into_iter().cloned().collect(),
        Some(list) => {
            let list: HashSet<&String> = list.iter().collect();
            allowed
                .into_iter()
                .filter(|name| list.contains(name))
                .cloned()
                .collect()
        }
    }
}

/// The names to hand [`agent::Agent::register_skills_from_dir_denying`] for one
/// chat — everything discovered that this chat should *not* receive.
///
/// The agent crate is read-only and its registration API takes what to skip
/// rather than what to keep, so the allow-list has to be inverted here. `None`
/// means "inherit", which is the workspace's own call and not this chat's.
pub(crate) fn session_deny(
    discovered: &[String],
    denied: &[String],
    active: Option<&[String]>,
) -> Option<Vec<String>> {
    active?;
    let keep = effective(discovered, denied, active);
    let keep: HashSet<String> = keep.into_iter().collect();
    Some(
        discovered
            .iter()
            .filter(|name| !keep.contains(*name))
            .cloned()
            .collect(),
    )
}

/// The pane's answer for one chat: what it carries, what it could carry, and
/// what the workspace keeps from it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ActivationReport {
    /// The caller's clock, echoed back.
    pub(crate) now_ms: u64,
    /// The chat this report is about.
    pub(crate) session: String,
    /// False when the chat inherits the workspace rather than holding a record.
    pub(crate) explicit: bool,
    /// What this chat's agent receives, by name.
    pub(crate) active: Vec<String>,
    /// What this chat may switch on: discovered minus the workspace's denials.
    pub(crate) available: Vec<String>,
    /// The workspace's own denials, named so the pane can say why a skill it
    /// can see is not offered here.
    pub(crate) denied: Vec<String>,
    /// The activation file.
    pub(crate) path: String,
    /// How many chats hold a record.
    pub(crate) sessions_known: usize,
    /// How many chats the file will remember.
    pub(crate) max_sessions: usize,
}

/// Build the report for one chat from the directory and the record.
pub(crate) fn activation_report(
    dir: &Path,
    denied: &[String],
    session: &str,
    activation: &Activation,
    path: &Path,
    now_ms: u64,
) -> ActivationReport {
    let (_, listed) = list(dir, denied);
    let discovered: Vec<String> = listed.iter().map(|skill| skill.name.clone()).collect();
    let active = activation.active(session);
    ActivationReport {
        now_ms,
        session: session.to_string(),
        explicit: activation.is_explicit(session),
        active: effective(&discovered, denied, active),
        available: effective(&discovered, denied, None),
        denied: discovered
            .iter()
            .filter(|name| denied.iter().any(|off| off == *name))
            .cloned()
            .collect(),
        path: path.display().to_string(),
        sessions_known: activation.record_count(),
        max_sessions: MAX_SESSIONS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A directory of this test's own, removed first so a previous run cannot
    /// answer for this one.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bos-skill-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn loader_reads(dir: &Path) -> Vec<crate::skills::Skill> {
        list(dir, &[]).1
    }

    /// A record file of this test's own, removed first and at the end: an
    /// assertion about a file is an assertion about the whole file.
    fn temp_record(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("bos-activation-{tag}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn a_chat_with_no_record_inherits_the_workspace_and_asks_the_agent_for_nothing() {
        let discovered = names(&["a", "b"]);

        let active = effective(&discovered, &[], None);
        assert_eq!(active, discovered);
        assert_eq!(
            session_deny(&discovered, &[], None),
            None,
            "no record, no call"
        );

        let record = Activation::default();
        assert!(!record.is_explicit("s1"));
        assert_eq!(record.active("s1"), None);
        assert_eq!(record.record_count(), 0);
    }

    #[test]
    fn switching_one_skill_off_in_a_chat_that_inherits_keeps_the_others() {
        let available = names(&["a", "b", "c"]);
        let mut record = Activation::default();
        assert!(
            record.set("s1", "b", false, &available).expect("changed"),
            "the first mutation is a change"
        );
        assert!(record.is_explicit("s1"));
        assert_eq!(record.active("s1"), Some(&names(&["a", "c"])[..]));
        // The allow-list is inverted for the agent, which takes what to skip.
        assert_eq!(
            session_deny(&available, &[], record.active("s1")),
            Some(names(&["b"]))
        );
        // And switching it back on leaves the chat carrying everything again,
        // which is *not* the same as forgetting the record: it is still this
        // chat's list.
        assert!(record.set("s1", "b", true, &available).expect("changed"));
        assert_eq!(record.active("s1"), Some(&available[..]));
        assert!(record.is_explicit("s1"));
        // Setting what is already set changes nothing, so no rebuild is owed.
        assert!(!record.set("s1", "b", true, &available).expect("no-op"));
        assert!(!record.set("s1", "zz", false, &available).expect("no-op"));
        // Switching *on* a skill an inheriting chat already carries is a no-op
        // that leaves no record behind: recording it would freeze the chat's set
        // where it stands, so a skill added to the directory later would stop
        // reaching a chat whose owner never asked for anything to change.
        let mut fresh = Activation::default();
        assert!(!fresh.set("s9", "a", true, &available).expect("no-op"));
        assert!(!fresh.is_explicit("s9"), "no record from a no-op");
        assert_eq!(fresh.record_count(), 0);
    }

    #[test]
    fn a_workspace_denial_is_a_floor_for_every_chat() {
        let discovered = names(&["a", "b", "c"]);
        let denied = names(&["b"]);

        // Inheriting already respects the workspace's switch-off.
        assert_eq!(effective(&discovered, &denied, None), names(&["a", "c"]));
        // And naming it in a chat's record cannot switch it back on.
        let asked = names(&["a", "b"]);
        assert_eq!(
            effective(&discovered, &denied, Some(&asked)),
            names(&["a"]),
            "the workspace keeps it off"
        );
        assert_eq!(
            session_deny(&discovered, &denied, Some(&asked)),
            Some(names(&["b", "c"]))
        );
    }

    #[test]
    fn a_name_the_workspace_does_not_offer_is_refused() {
        let available = names(&["a", "b"]);
        let mut record = Activation::default();
        let err = record
            .set("s1", "nope", true, &available)
            .expect_err("refused");
        assert!(
            err.contains("does not offer") && err.contains("nope"),
            "{err}"
        );
        assert!(!record.is_explicit("s1"), "a refusal leaves no record");

        let blank = record
            .set("  ", "a", true, &available)
            .expect_err("refused");
        assert!(blank.contains("a chat is needed"), "{blank}");
    }

    #[test]
    fn an_explicit_empty_record_is_not_the_same_as_inheriting() {
        let available = names(&["a", "b"]);
        let mut record = Activation::default();
        record.set("s1", "a", false, &available).expect("changed");
        let last = record.set("s1", "b", false, &available).expect("changed");
        assert!(last);
        assert!(record.is_explicit("s1"));
        assert_eq!(record.active("s1"), Some(&[][..]), "carries nothing");
        assert_eq!(
            effective(&available, &[], record.active("s1")),
            Vec::<String>::new()
        );
        assert_eq!(
            session_deny(&available, &[], record.active("s1")),
            Some(available.clone())
        );

        // Clearing is what returns the chat to the workspace.
        assert!(record.clear("s1"));
        assert!(!record.is_explicit("s1"));
        assert!(!record.clear("s1"), "nothing left to clear");
        assert_eq!(effective(&available, &[], record.active("s1")), available);
        assert_eq!(session_deny(&available, &[], record.active("s1")), None);
    }

    #[test]
    fn a_record_naming_a_skill_the_directory_lost_is_ignored() {
        let discovered = names(&["a"]);
        let stale = names(&["gone", "a"]);
        assert_eq!(
            effective(&discovered, &[], Some(&stale)),
            names(&["a"]),
            "a name the directory no longer holds is not invented"
        );
        assert_eq!(session_deny(&discovered, &[], Some(&stale)), Some(vec![]));
    }

    #[test]
    fn the_caps_are_named_when_they_bite() {
        let available = names(&["a", "b"]);
        let mut record = Activation::default();
        for i in 0..MAX_SESSIONS {
            record
                .set(&format!("s{i}"), "b", false, &available)
                .expect("within the cap");
        }
        let err = record
            .set("one-too-many", "a", true, &available)
            .expect_err("refused");
        assert!(err.contains(&MAX_SESSIONS.to_string()), "{err}");
        // An existing chat may still change, so the cap cannot lock the app.
        assert!(record.set("s0", "b", true, &available).expect("changed"));

        // A workspace with more skills than a hand-edited file may carry:
        // switching any of them off is still allowed, because the record's size
        // is the workspace's business and not a constant.
        let many: Vec<String> = (0..MAX_NAMES_PER_RECORD + 40)
            .map(|i| format!("k{i:04}"))
            .collect();
        let mut wide = Activation::default();
        assert!(wide.set("s1", "k0000", false, &many).expect("changed"));
        assert_eq!(wide.active("s1").map(<[String]>::len), Some(many.len() - 1));
    }

    #[test]
    fn a_record_file_round_trips_and_another_shape_reads_as_empty() {
        let path = temp_record("round-trip");
        let available = names(&["a", "b"]);
        let mut record = Activation::default();
        record.set("s1", "b", false, &available).expect("changed");
        record.set("s2", "b", false, &available).expect("changed");
        record.save(&path).expect("saved");

        let read = Activation::load(&path);
        assert_eq!(read.record_count(), 2);
        assert_eq!(read.active("s1"), Some(&names(&["a"])[..]));
        assert_eq!(read.active("s2"), Some(&names(&["a"])[..]));
        assert!(read.is_explicit("s1") && read.is_explicit("s2"));

        // Another shape's file, and a hand-edited one, both read as "inherit":
        // losing an activation is smaller than refusing to open a chat.
        std::fs::write(&path, r#"{"version": 99, "sessions": {"s1": ["a"]}}"#).expect("wrote");
        assert_eq!(Activation::load(&path).record_count(), 0);
        std::fs::write(&path, r#"{"sessions": {"s1": ["a"]}}"#).expect("wrote");
        assert_eq!(Activation::load(&path).record_count(), 0);
        std::fs::write(&path, "not json at all").expect("wrote");
        assert_eq!(Activation::load(&path).record_count(), 0);
        // A hand-edited file that is the right shape still gets cleaned up.
        std::fs::write(
            &path,
            r#"{"version": 1, "sessions": {"  ": ["a"], "s1": ["b", " b ", "", "b", "a"]}}"#,
        )
        .expect("wrote");
        let read = Activation::load(&path);
        assert!(
            read.is_explicit("s1") && !read.is_explicit("  "),
            "a blank chat is not a chat"
        );
        // A hand-edited record longer than the bound is cut on the way in.
        let long: Vec<String> = (0..MAX_NAMES_PER_RECORD + 10)
            .map(|i| format!("n{i:04}"))
            .collect();
        let file = serde_json::json!({"version": 1, "sessions": {"s1": long}});
        std::fs::write(&path, file.to_string()).expect("wrote");
        assert_eq!(
            Activation::load(&path).active("s1").map(<[String]>::len),
            Some(MAX_NAMES_PER_RECORD)
        );
        assert_eq!(read.active("s1"), Some(&names(&["a", "b"])[..]));

        let _ = std::fs::remove_file(&path);
        assert!(!path.exists(), "the test owns its file");
    }

    #[test]
    fn a_saved_skill_is_the_file_the_loader_reads_back() {
        let dir = temp_dir("round-trip");
        let saved = save(
            &dir,
            "code-review",
            "Review a diff",
            "Read the diff, then list findings.",
            false,
        )
        .expect("saved");
        assert_eq!(
            saved.path,
            dir.join("code-review")
                .join("SKILL.md")
                .display()
                .to_string()
        );
        assert!(!saved.replaced);
        let body = "Read the diff, then list findings.";
        assert_eq!(saved.instructions_chars, body.chars().count());
        assert!(std::fs::read_to_string(&saved.path)
            .expect("file")
            .starts_with("---\nname: \"code-review\"\n"));

        let rows = loader_reads(&dir);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].name, "code-review");
        assert_eq!(rows[0].description, "Review a diff");
        // The body the model receives is the body that was authored.
        assert_eq!(rows[0].instructions_chars, body.chars().count());
        let mut loader = agent::skills::SkillLoader::new(dir.clone());
        loader.discover().expect("discover");
        let content = loader.load("code-review").expect("loaded");
        assert_eq!(content.instructions, body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_description_with_yaml_metacharacters_survives() {
        let dir = temp_dir("yaml");
        let description = "Step 1: do \"it\" — #now (see: notes)";
        save(&dir, "quoted", description, "Body.", false).expect("saved");
        let rows = loader_reads(&dir);
        assert_eq!(
            rows[0].description, description,
            "the scalar split the document"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_name_that_would_leave_the_directory_is_refused() {
        let dir = temp_dir("escape");
        // The name this test tries to escape *to* carries the process id, so a
        // leftover from an earlier run (or from a run whose guard was removed
        // on purpose) cannot make this assertion true or false for the wrong
        // reason: the check owns the path it names.
        let stray = format!("evil-{}", std::process::id());
        let escape = format!("../{stray}");
        let target = dir.parent().unwrap_or(Path::new(".")).join(&stray);
        let _ = std::fs::remove_dir_all(&target);
        for name in [escape.as_str(), "a/b", "..", ".", "", "  ", "a\\b"] {
            let err = save(&dir, name, "d", "b", false).expect_err("must be refused");
            assert!(
                err.contains("name:") || err.contains("must be"),
                "{name:?}: {err}"
            );
        }
        assert!(!dir.exists(), "a refused name created something anyway");
        // And the containment is real, not just a message: the parent is intact.
        assert!(
            !target.exists(),
            "a refused name wrote {}",
            target.display()
        );
        let _ = std::fs::remove_dir_all(&target);
    }

    #[test]
    fn the_agent_supplies_the_name_and_description_rules() {
        let blank = validate("", "", "").expect_err("refused");
        assert!(
            blank.iter().any(|e| e == "name: must not be empty"),
            "{blank:?}"
        );
        assert!(
            blank.iter().any(|e| e == "description: must not be empty"),
            "{blank:?}"
        );
        assert!(
            blank.iter().any(|e| e == "body: must not be empty"),
            "{blank:?}"
        );

        let spaced = validate("two words", "d", "b").expect_err("refused");
        assert!(
            spaced
                .iter()
                .any(|e| e == "name: must not contain whitespace"),
            "{spaced:?}"
        );

        let long_name = "n".repeat(agent::skills::SkillAuthoringRecord::MAX_NAME_LEN + 1);
        let long = validate(&long_name, "d", "b").expect_err("refused");
        assert!(
            long.iter()
                .any(|e| e.starts_with("name: must be at most 128")),
            "{long:?}"
        );

        let long_desc = "d".repeat(agent::skills::SkillAuthoringRecord::MAX_DESCRIPTION_LEN + 1);
        let long = validate("ok", &long_desc, "b").expect_err("refused");
        assert!(
            long.iter()
                .any(|e| e.starts_with("description: must be at most 1024")),
            "{long:?}"
        );

        let long_body = "b".repeat(MAX_BODY_CHARS + 1);
        let long = validate("ok", "d", &long_body).expect_err("refused");
        assert!(
            long.iter()
                .any(|e| e.starts_with("body: must be at most 12000")),
            "{long:?}"
        );

        assert!(validate("ok", "d", "b").is_ok());
    }

    #[test]
    fn an_existing_skill_is_kept_unless_the_caller_asks_to_replace_it() {
        let dir = temp_dir("replace");
        save(&dir, "keep", "first", "Original body.", false).expect("saved");
        let err = save(&dir, "keep", "second", "Replacement body.", false).expect_err("refused");
        assert!(err.contains("already exists"), "{err}");
        let rows = loader_reads(&dir);
        assert_eq!(rows[0].description, "first", "a refusal changed the file");

        let saved = save(&dir, "keep", "second", "Replacement body.", true).expect("replaced");
        assert!(saved.replaced);
        let rows = loader_reads(&dir);
        assert_eq!(rows[0].description, "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_listing_is_what_the_agent_receives_and_says_who_is_denied() {
        let dir = temp_dir("listing");
        save(&dir, "alpha", "first skill", "A".repeat(11).as_str(), false).expect("alpha");
        save(&dir, "beta", "second skill", "bb", false).expect("beta");
        let (exists, rows) = list(&dir, &["beta".to_string()]);
        assert!(exists);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "alpha", "sorted by name");
        assert_eq!(rows[0].instructions_chars, 11, "counts what the agent gets");
        assert!(!rows[0].denied);
        assert!(rows[1].denied, "the workspace deny-list reaches the row");

        let report = report(&dir, &[], 7);
        assert_eq!(report.now_ms, 7);
        assert_eq!(report.dir, dir.display().to_string());
        assert_eq!(report.max_body_chars, MAX_BODY_CHARS);
        assert_eq!(report.max_name_chars, 128);
        assert_eq!(report.max_description_chars, 1024);
        assert_eq!(report.skills.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_directory_discovers_nothing_and_is_not_an_error() {
        let dir = temp_dir("missing");
        assert!(!dir.exists());
        let (exists, rows) = list(&dir, &[]);
        assert!(!exists);
        assert!(rows.is_empty());
        let report = report(&dir, &[], 0);
        assert!(!report.exists);
        assert!(report.skills.is_empty());
    }
}
