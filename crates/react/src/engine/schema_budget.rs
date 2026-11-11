//! A turn-scoped budget for every tool definition a request carries.
//!
//! The array the host assembled is priced first (`account_initial`), so the
//! definitions a request starts with and the ones discovery adds spend ONE
//! budget instead of two. A single find_tool result is also bounded, and a
//! model can run several searches in one turn and grow the live tool array step
//! by step; every definition actually added to the next request is priced, a
//! repeat of the same revision is deduplicated, and growth is refused with an
//! explicit report instead of silently dropping a schema.
//!
//! The initial definitions are deliberately NOT recorded as revealed: the
//! selection telemetry distinguishes a tool the request advertised from one
//! the model had to discover.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Default cumulative schema bytes per turn.
pub const DEFAULT_SCHEMA_BUDGET: usize = 64 * 1024;
/// Default schema bytes for one tool.
pub const DEFAULT_PER_TOOL_BUDGET: usize = 16 * 1024;

/// What happened to one definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaAccount {
    /// A new definition that now spends budget.
    Added {
        /// Bytes this definition spends.
        bytes: usize,
    },
    /// The same name and revision were already revealed; nothing was spent.
    AlreadyRevealed,
}

/// Why one definition could not join the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaRejection {
    /// One schema alone exceeds the per-tool limit.
    ToolTooLarge {
        /// Tool name.
        name: String,
        /// Serialized bytes.
        bytes: usize,
        /// Configured per-tool limit.
        limit: usize,
    },
    /// The cumulative turn budget has no room left.
    BudgetExhausted {
        /// Tool name.
        name: String,
        /// Bytes the definition would add.
        bytes: usize,
        /// Bytes already spent.
        spent: usize,
        /// Configured cumulative limit.
        limit: usize,
    },
}

/// Identity of a revealed definition.
///
/// The fingerprint changes when the schema or version changes, so a new
/// revision is priced again instead of reusing a stale definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaRevision(u64);

impl SchemaRevision {
    /// Compute a revision from a version string and the serialized schema.
    pub fn of(version: Option<&str>, schema_json: &str) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        version.hash(&mut hasher);
        schema_json.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// Per-turn schema accounting.
#[derive(Debug, Clone)]
pub struct SchemaBudget {
    limit: usize,
    per_tool: usize,
    spent: usize,
    revealed: BTreeMap<String, (SchemaRevision, usize)>,
}

impl Default for SchemaBudget {
    fn default() -> Self {
        Self::new(DEFAULT_SCHEMA_BUDGET, DEFAULT_PER_TOOL_BUDGET)
    }
}

impl SchemaBudget {
    /// Create a budget with explicit limits. Zero selects the default.
    pub fn new(limit: usize, per_tool: usize) -> Self {
        Self {
            limit: if limit == 0 {
                DEFAULT_SCHEMA_BUDGET
            } else {
                limit
            },
            per_tool: if per_tool == 0 {
                DEFAULT_PER_TOOL_BUDGET
            } else {
                per_tool
            },
            spent: 0,
            revealed: BTreeMap::new(),
        }
    }

    /// Forget every revealed definition. Called at the start of a turn.
    pub fn reset(&mut self) {
        self.spent = 0;
        self.revealed.clear();
    }

    /// Bytes spent this turn.
    pub fn spent(&self) -> usize {
        self.spent
    }

    /// Cumulative limit.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Per-tool limit.
    pub fn per_tool(&self) -> usize {
        self.per_tool
    }

    /// Whether discovery revealed this tool in the current turn.
    pub fn was_revealed(&self, name: &str) -> bool {
        self.revealed.contains_key(name)
    }

    /// Price one definition the request carries from the start.
    ///
    /// The initial array is admitted by the host under the same limit (see
    /// [`crate::tool::admission`]), so the two halves cannot add up to an
    /// unbounded request. Unlike `account`, this never refuses: a definition
    /// already in the request cannot be withheld here, and a required one is
    /// allowed to overdraw the limit. The spend saturates at the limit, which
    /// leaves every later reveal refused with the ordinary exhaustion report.
    ///
    /// The name is NOT recorded as revealed: the per-step request already
    /// advertises it, so calling it is not a discovery.
    pub fn account_initial(&mut self, _name: &str, bytes: usize) {
        self.spent = self.spent.saturating_add(bytes).min(self.limit);
    }

    /// Account one definition, or refuse it.
    ///
    /// Re-accounting the same name with the SAME revision is idempotent and
    /// costs nothing. A different revision replaces the old definition: only
    /// the size delta is charged, and a growth that would exceed the budget is
    /// refused without changing the recorded revision.
    pub fn account(
        &mut self,
        name: &str,
        revision: SchemaRevision,
        bytes: usize,
    ) -> Result<SchemaAccount, SchemaRejection> {
        if bytes > self.per_tool {
            return Err(SchemaRejection::ToolTooLarge {
                name: name.to_string(),
                bytes,
                limit: self.per_tool,
            });
        }
        if let Some((old_revision, old_bytes)) = self.revealed.get(name).copied() {
            if old_revision == revision {
                return Ok(SchemaAccount::AlreadyRevealed);
            }
            let growth = bytes.saturating_sub(old_bytes);
            if self.spent + growth > self.limit {
                return Err(SchemaRejection::BudgetExhausted {
                    name: name.to_string(),
                    bytes: growth,
                    spent: self.spent,
                    limit: self.limit,
                });
            }
            self.spent = self.spent.saturating_sub(old_bytes) + bytes;
            self.revealed.insert(name.to_string(), (revision, bytes));
            return Ok(SchemaAccount::Added { bytes });
        }
        if self.spent + bytes > self.limit {
            return Err(SchemaRejection::BudgetExhausted {
                name: name.to_string(),
                bytes,
                spent: self.spent,
                limit: self.limit,
            });
        }
        self.spent += bytes;
        self.revealed.insert(name.to_string(), (revision, bytes));
        Ok(SchemaAccount::Added { bytes })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(tag: &str) -> SchemaRevision {
        SchemaRevision::of(Some("1.0.0"), tag)
    }

    #[test]
    fn repeated_reveals_are_idempotent_and_a_new_revision_charges_again() {
        let mut budget = SchemaBudget::new(100, 60);
        assert_eq!(
            budget.account("a", revision("schema-a"), 40),
            Ok(SchemaAccount::Added { bytes: 40 })
        );
        assert_eq!(
            budget.account("a", revision("schema-a"), 40),
            Ok(SchemaAccount::AlreadyRevealed)
        );
        assert_eq!(budget.spent(), 40, "a repeat spends nothing");
        assert!(budget.account("a", revision("schema-a-v2"), 50).is_ok());
        assert_eq!(budget.spent(), 50, "a revision charges its delta");
    }

    #[test]
    fn a_single_oversized_schema_is_refused() {
        let mut budget = SchemaBudget::new(100, 30);
        assert_eq!(
            budget.account("big", revision("big"), 31),
            Err(SchemaRejection::ToolTooLarge {
                name: "big".to_string(),
                bytes: 31,
                limit: 30,
            })
        );
        assert_eq!(budget.spent(), 0);
    }

    #[test]
    fn the_cumulative_budget_refuses_without_mutating_state() {
        let mut budget = SchemaBudget::new(50, 40);
        assert!(budget.account("a", revision("a"), 30).is_ok());
        assert_eq!(
            budget.account("b", revision("b"), 30),
            Err(SchemaRejection::BudgetExhausted {
                name: "b".to_string(),
                bytes: 30,
                spent: 30,
                limit: 50,
            })
        );
        assert_eq!(budget.spent(), 30, "a refusal spends nothing");
        assert!(budget.account("c", revision("c"), 20).is_ok());
        assert_eq!(budget.spent(), 50);
    }

    #[test]
    fn reset_starts_a_fresh_turn() {
        let mut budget = SchemaBudget::new(50, 40);
        budget.account("a", revision("a"), 30).expect("first");
        budget.reset();
        assert_eq!(budget.spent(), 0);
        assert!(budget.account("a", revision("a"), 30).is_ok());
    }

    /// The array a request starts with spends the same budget a discovery does,
    /// and it is not a discovery: the selection signal must not read it as one.
    #[test]
    fn initial_definitions_spend_budget_without_counting_as_revealed() {
        let mut budget = SchemaBudget::new(100, 70);
        budget.account_initial("bash", 40);
        assert_eq!(budget.spent(), 40);
        assert!(
            !budget.was_revealed("bash"),
            "an advertised tool is not a discovered one"
        );
        assert_eq!(
            budget.account("deploy", revision("deploy"), 61),
            Err(SchemaRejection::BudgetExhausted {
                name: "deploy".to_string(),
                bytes: 61,
                spent: 40,
                limit: 100,
            }),
            "a reveal cannot exceed the room the initial array left"
        );
    }

    /// A required definition may overdraw the limit; the spend saturates, so
    /// every later reveal is refused with the ordinary report rather than
    /// underflowing the arithmetic.
    #[test]
    fn an_initial_overdraft_saturates_instead_of_wrapping() {
        let mut budget = SchemaBudget::new(50, 60);
        budget.account_initial("huge", 80);
        assert_eq!(budget.spent(), 50);
        assert!(matches!(
            budget.account("later", revision("later"), 1),
            Err(SchemaRejection::BudgetExhausted { .. })
        ));
    }
}
