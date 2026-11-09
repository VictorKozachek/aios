//! Context builder with a token budget (docs/PROTOTYPE.md §4).
//!
//! The context is assembled deterministically: the workspace system prompt,
//! pinned facts with provenance, the tail of the session, and the retrieved
//! fragments (episodes, facts, file chunks). Every part has a share of the
//! budget, and the resulting plan reports what was included, what was dropped
//! and how many tokens each part took — the console shows this in `/status`.
//!
//! Token counts come from the backend (`chars/4` estimate in the prototype),
//! so the builder takes a counter function instead of a tokenizer.

use aios_modeld::Message;

use crate::bm25::{RefKind, SearchHit};
use crate::fact::Fact;

/// How many characters of one retrieved fragment are pasted into the context.
pub const MAX_HIT_CHARS: usize = 400;

/// Share of the free budget the facts block may take, percent.
pub const FACTS_SHARE: usize = 30;

/// Share of the free budget the retrieved block may take, percent.
pub const RETRIEVED_SHARE: usize = 30;

/// Which part of the budget a piece of context belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Breakdown {
    /// Workspace system prompt.
    pub system: usize,
    /// Pinned and retrieved facts with provenance.
    pub facts: usize,
    /// Tail of the session dialogue.
    pub tail: usize,
    /// Retrieved episodes and file chunks.
    pub retrieved: usize,
}

impl Breakdown {
    /// Sum of all parts.
    pub fn total(&self) -> usize {
        self.system + self.facts + self.tail + self.retrieved
    }
}

/// Result of one context assembly.
#[derive(Debug, Clone)]
pub struct ContextPlan {
    /// Messages to send to the model.
    pub messages: Vec<Message>,
    /// Token count per part.
    pub breakdown: Breakdown,
    /// Token budget the plan was built for.
    pub budget: usize,
    /// History messages dropped because they did not fit.
    pub dropped_messages: usize,
    /// Retrieved hits dropped because they did not fit.
    pub dropped_hits: usize,
    /// Facts dropped because they did not fit.
    pub dropped_facts: usize,
    /// Retrieved hits truncated by [`MAX_HIT_CHARS`].
    pub truncated_hits: usize,
    /// Fact ids that went into the context.
    pub facts_included: Vec<String>,
    /// Retrieved hits that went into the context.
    pub hits_included: Vec<SearchHit>,
    /// Labels of file fragments in the context (`files/report.md §2`).
    pub file_refs: Vec<String>,
}

impl ContextPlan {
    /// Total tokens of the plan.
    pub fn total(&self) -> usize {
        self.breakdown.total()
    }

    /// `true` if the plan fits the budget.
    pub fn fits_budget(&self) -> bool {
        self.total() <= self.budget
    }

    /// One-line composition for `/status`:
    /// `system 120 · facts 80 · tail 400 · retrieved 200 (из 3000)`.
    pub fn composition(&self) -> String {
        format!(
            "system {} · facts {} · tail {} · retrieved {} (из {})",
            self.breakdown.system,
            self.breakdown.facts,
            self.breakdown.tail,
            self.breakdown.retrieved,
            self.budget
        )
    }

    /// `true` if anything had to be shortened or dropped.
    pub fn was_truncated(&self) -> bool {
        self.dropped_messages > 0
            || self.dropped_hits > 0
            || self.dropped_facts > 0
            || self.truncated_hits > 0
    }
}

/// Builds the context for one model turn.
///
/// `pinned_facts` are the facts that must be visible (the prototype pins every
/// human fact); `hits` are the BM25 results for the user message; `tail` is the
/// session history in chronological order. The system prompt is never dropped:
/// it is cheaper to answer with less memory than without instructions.
pub fn build_context(
    system_prompt: &str,
    budget: usize,
    pinned_facts: &[Fact],
    hits: &[SearchHit],
    tail: &[Message],
    count_tokens: &dyn Fn(&str) -> usize,
) -> ContextPlan {
    let system_tokens = count_tokens(system_prompt);
    let remaining = budget.saturating_sub(system_tokens);
    let facts_cap = remaining * FACTS_SHARE / 100;
    let retrieved_cap = remaining * RETRIEVED_SHARE / 100;
    let tail_cap = remaining - facts_cap - retrieved_cap;

    let mut facts_included = Vec::new();
    let mut facts_block = String::from("Память воркспейса (провенанс):\n");
    let mut facts_tokens = count_tokens(&facts_block);
    let mut dropped_facts = 0_usize;

    for fact in pinned_facts {
        let line = format!(
            "- [fact:{}] {}/{} = {} ({})\n",
            fact.id,
            fact.subject,
            fact.key,
            fact.value,
            fact.provenance()
        );
        let cost = count_tokens(&line);
        if facts_tokens + cost > facts_cap {
            dropped_facts += 1;
            continue;
        }
        facts_block.push_str(&line);
        facts_tokens += cost;
        facts_included.push(fact.id.clone());
    }
    for hit in hits.iter().filter(|hit| hit.ref_kind == RefKind::Fact) {
        if facts_included.contains(&hit.id) {
            continue;
        }
        let line = format!("- [{}] {} ({})\n", hit.label, hit.text, hit.provenance);
        let cost = count_tokens(&line);
        if facts_tokens + cost > facts_cap {
            dropped_facts += 1;
            continue;
        }
        facts_block.push_str(&line);
        facts_tokens += cost;
        facts_included.push(hit.id.clone());
    }

    let mut retrieved_block = String::from("Извлечённые фрагменты:\n");
    let mut retrieved_tokens = count_tokens(&retrieved_block);
    let mut hits_included = Vec::new();
    let mut file_refs = Vec::new();
    let mut dropped_hits = 0_usize;
    let mut truncated_hits = 0_usize;
    for hit in hits.iter().filter(|hit| hit.ref_kind != RefKind::Fact) {
        let (text, truncated) = truncate_chars(&hit.text, MAX_HIT_CHARS);
        if truncated {
            truncated_hits += 1;
        }
        let block = format!("[{}] {}\n", hit.label, text);
        let cost = count_tokens(&block);
        if retrieved_tokens + cost > retrieved_cap {
            dropped_hits += 1;
            continue;
        }
        retrieved_block.push_str(&block);
        retrieved_tokens += cost;
        if hit.ref_kind == RefKind::File {
            file_refs.push(hit.label.clone());
        }
        // Keep the exact text that went into the context (possibly truncated).
        let mut included = hit.clone();
        included.text = text;
        hits_included.push(included);
    }

    let mut selected: Vec<Message> = Vec::new();
    let mut tail_tokens = 0_usize;
    let mut dropped_messages = 0_usize;
    for message in tail.iter().rev() {
        let cost = count_tokens(&message.content) + 4;
        if tail_tokens + cost > tail_cap {
            dropped_messages += 1;
            continue;
        }
        tail_tokens += cost;
        selected.push(message.clone());
    }
    selected.reverse();

    let mut messages = Vec::with_capacity(selected.len() + 3);
    messages.push(Message::system(system_prompt));
    if !facts_included.is_empty() {
        facts_block.pop();
        messages.push(Message::system(facts_block));
    } else {
        facts_tokens = 0;
    }
    if !hits_included.is_empty() {
        retrieved_block.pop();
        messages.push(Message::system(retrieved_block));
    } else {
        retrieved_tokens = 0;
    }
    messages.extend(selected);

    ContextPlan {
        messages,
        breakdown: Breakdown {
            system: system_tokens,
            facts: facts_tokens,
            tail: tail_tokens,
            retrieved: retrieved_tokens,
        },
        budget,
        dropped_messages,
        dropped_hits,
        dropped_facts,
        truncated_hits,
        facts_included,
        hits_included,
        file_refs,
    }
}

fn truncate_chars(text: &str, limit: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= limit {
        return (text.to_string(), false);
    }
    let mut shortened: String = text.chars().take(limit).collect();
    shortened.push('…');
    (shortened, true)
}

#[cfg(test)]
mod tests {
    use super::{Breakdown, build_context};
    use crate::bm25::{RefKind, SearchHit};
    use crate::fact::{Fact, Source, human_fact};
    use aios_modeld::Message;

    /// `chars/4` estimate, the same rule the model layer uses.
    fn count(text: &str) -> usize {
        if text.is_empty() {
            0
        } else {
            text.chars().count().div_ceil(4)
        }
    }

    fn fact(subject: &str, key: &str, value: &str) -> Fact {
        human_fact(
            "work",
            subject,
            key,
            value,
            Source {
                session: "20261109-190000".into(),
                seq: 3,
            },
        )
    }

    fn file_hit(label: &str, text: &str) -> SearchHit {
        SearchHit {
            id: format!("file:{label}#0"),
            ref_kind: RefKind::File,
            label: label.to_string(),
            provenance: label.to_string(),
            text: text.to_string(),
            score: 1.0,
        }
    }

    fn episode_hit(seq: u64, text: &str) -> SearchHit {
        SearchHit {
            id: format!("ep:1#{seq}"),
            ref_kind: RefKind::Episode,
            label: format!("сессия 1 #{seq}"),
            provenance: "сессия 1".into(),
            text: text.to_string(),
            score: 0.9,
        }
    }

    #[test]
    fn system_and_pinned_facts_are_always_included() {
        let plan = build_context(
            "Ты — AIOS.",
            1000,
            &[fact("project", "format", "сегменты")],
            &[],
            &[Message::user("привет")],
            &count,
        );
        assert_eq!(plan.messages[0].content, "Ты — AIOS.");
        assert!(
            plan.messages[1]
                .content
                .contains("project/format = сегменты")
        );
        assert!(plan.messages[1].content.contains("автор: human"));
        assert_eq!(plan.facts_included.len(), 1);
        assert!(plan.fits_budget());
    }

    #[test]
    fn retrieved_files_and_episodes_go_into_the_context() {
        let plan = build_context(
            "system",
            1000,
            &[],
            &[
                file_hit("files/report.md §2", "фрагмент про снапшоты"),
                episode_hit(7, "обсуждали формат"),
            ],
            &[],
            &count,
        );
        assert!(
            plan.messages
                .iter()
                .any(|message| message.content.contains("[files/report.md §2]"))
        );
        assert_eq!(plan.file_refs, vec!["files/report.md §2"]);
        assert_eq!(plan.hits_included.len(), 2);
        assert!(plan.breakdown.retrieved > 0);
        assert!(plan.fits_budget());
    }

    #[test]
    fn long_hits_are_truncated_to_the_limit() {
        let long_text = "ф".repeat(super::MAX_HIT_CHARS * 2);
        let plan = build_context(
            "system",
            2000,
            &[],
            &[file_hit("files/big.md §1", &long_text)],
            &[],
            &count,
        );
        assert_eq!(plan.truncated_hits, 1);
        let included = &plan.hits_included[0];
        assert!(included.text.chars().count() <= super::MAX_HIT_CHARS + 1);
        assert!(included.text.ends_with('…'));
    }

    #[test]
    fn budget_is_never_exceeded_on_long_history() {
        let tail: Vec<Message> = (0..200)
            .map(|index| {
                if index % 2 == 0 {
                    Message::user(format!("вопрос номер {index} {}", "текст ".repeat(30)))
                } else {
                    Message::assistant(format!("ответ номер {index} {}", "текст ".repeat(30)))
                }
            })
            .collect();
        let plan = build_context("system prompt", 400, &[], &[], &tail, &count);
        assert!(plan.fits_budget(), "total {} > budget 400", plan.total());
        assert!(plan.dropped_messages > 0);
        assert!(plan.was_truncated());
        assert!(plan.messages.len() <= 400 / 4);
    }

    #[test]
    fn tiny_budget_keeps_the_system_prompt_only() {
        let plan = build_context(
            "системный промпт",
            5,
            &[fact("s", "k", "v")],
            &[file_hit("files/a.md §1", "текст")],
            &[Message::user("привет")],
            &count,
        );
        assert_eq!(plan.messages.len(), 1);
        assert_eq!(
            plan.breakdown,
            Breakdown {
                system: 4,
                facts: 0,
                tail: 0,
                retrieved: 0
            }
        );
        assert!(plan.fits_budget());
    }

    #[test]
    fn composition_line_shows_every_part() {
        let plan = build_context(
            "system",
            500,
            &[fact("s", "k", "v")],
            &[file_hit("files/a.md §1", "текст")],
            &[Message::user("вопрос")],
            &count,
        );
        let line = plan.composition();
        for part in ["system", "facts", "tail", "retrieved", "из 500"] {
            assert!(line.contains(part), "{line}");
        }
    }
}
