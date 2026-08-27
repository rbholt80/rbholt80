//! Which model does which work.
//!
//! One model doing everything is the arrangement you get by default and the
//! wrong one on a machine that has to run other programs too. Most of what
//! this system asks a model for is small: name this change in six words, is
//! this sentence an instruction, which of these folders does this file belong
//! in. That work does not need a seven-billion-parameter model any more than
//! adding two numbers needs a spreadsheet, and on a laptop the difference
//! between a one-and-a-half-billion model and a seven is the difference
//! between the machine being usable while it thinks and not.
//!
//! So the work is named, and each name says what size of model it needs and
//! why. Three rules hold the arrangement together:
//!
//! * **Small handles the routine.** Naming, classifying, summarising,
//!   extracting — where the answer is short, the input is short, and being
//!   wrong is cheap.
//! * **Large handles the reasoning.** Turning a sentence into a plan of steps
//!   against real capabilities, and anything where being wrong is expensive.
//! * **Nothing that decides whether an action is safe goes to a small model,
//!   or to any model at all.** Risk is decided by the policy, from a table.
//!   A model's opinion about whether deleting your photographs is dangerous
//!   is not evidence, and asking one invites the answer to be talked round.

use crate::router::Tier;

/// A kind of work a model is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// Turn a sentence into a plan of steps against real capabilities.
    ///
    /// The hardest thing here and the only one where a wrong answer is
    /// expensive: a bad plan is shown to a person who may say yes to it.
    Plan,
    /// Answer a question in prose, at whatever length it needs.
    Converse,
    /// Say what a change was, in a few words, for the ledger.
    Describe,
    /// Decide whether typed words are an instruction or a name being looked
    /// for. A wrong answer costs one arrow key.
    Classify,
    /// Which family or folder something belongs in.
    Sort,
    /// Reduce something already written to something shorter.
    Summarise,
    /// Pull a name, a date or a path out of a sentence.
    Extract,
}

impl Job {
    /// Which size of model this work needs.
    pub fn tier(self) -> Tier {
        match self {
            // Short answers to short questions, where being wrong is cheap
            // and recoverable.
            Job::Describe | Job::Classify | Job::Sort | Job::Summarise | Job::Extract => {
                Tier::Small
            }
            // Reasoning over the capability vocabulary, or writing something a
            // person will read as an answer.
            Job::Plan | Job::Converse => Tier::Large,
        }
    }

    /// How long an answer to expect. A small model given a large budget will
    /// use it, and a six-word description that arrives as four paragraphs has
    /// cost more than it saved.
    pub fn max_tokens(self) -> u64 {
        match self {
            Job::Classify => 8,
            Job::Sort | Job::Extract => 48,
            Job::Describe => 64,
            Job::Summarise => 256,
            Job::Converse => 1024,
            Job::Plan => 2048,
        }
    }

    /// Whether this work may fall back to a larger model when no small one is
    /// available.
    ///
    /// All of it may. The tier is about spending the least that will do, not
    /// about refusing to work: a machine with only a large model should still
    /// be able to name a change.
    pub fn may_escalate(self) -> bool {
        true
    }

    /// Read a job name as a caller would type it.
    pub fn named(s: &str) -> Option<Job> {
        match s.to_ascii_lowercase().as_str() {
            "plan" => Some(Job::Plan),
            "converse" | "chat" | "answer" => Some(Job::Converse),
            "describe" => Some(Job::Describe),
            "classify" => Some(Job::Classify),
            "sort" | "categorise" | "categorize" => Some(Job::Sort),
            "summarise" | "summarize" => Some(Job::Summarise),
            "extract" => Some(Job::Extract),
            _ => None,
        }
    }

    /// The name used in logs and in `model.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            Job::Plan => "plan",
            Job::Converse => "converse",
            Job::Describe => "describe",
            Job::Classify => "classify",
            Job::Sort => "sort",
            Job::Summarise => "summarise",
            Job::Extract => "extract",
        }
    }
}

/// Whether a model may be asked this at all.
///
/// The one hard line. Risk comes from the capability table in policy, and a
/// model is never consulted about it — not the large one either. Two reasons,
/// and the second is the important one:
///
/// * A model's answer varies with how the question was phrased, and "is this
///   dangerous" is exactly the question an attacker phrases carefully.
/// * A risk table can be read. A judgement cannot, and a system whose safety
///   rules cannot be read is one nobody can check.
///
/// This is the written form of a rule the code already keeps by construction:
/// a plan names capabilities, and the risk of a capability comes from the
/// table in policy. `broker::tests` proves it — a step that states its own
/// risk, in its arguments and in its summary, does not change the verdict.
/// Keeping the rule written down as well means the next person to add a
/// model call has somewhere to check before adding it.
#[cfg_attr(not(test), allow(dead_code))]
pub fn may_ask_a_model(question: Decision) -> bool {
    match question {
        Decision::WhatToDo | Decision::HowToSayIt | Decision::WhereItBelongs => true,
        Decision::WhetherItIsSafe | Decision::WhetherToAllowIt => false,
    }
}

/// The kinds of decision this system makes, split by who gets to make them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub enum Decision {
    /// What steps would satisfy this request.
    WhatToDo,
    /// How to word something for a person to read.
    HowToSayIt,
    /// Which folder or family something goes in.
    WhereItBelongs,
    /// How dangerous an action is.
    WhetherItIsSafe,
    /// Whether this subject may perform this capability here.
    WhetherToAllowIt,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_routine_work_goes_to_the_small_model() {
        // Most of what this system asks for is short answers to short
        // questions, and sending all of it to the large model is what makes a
        // laptop unusable while it thinks.
        for j in [
            Job::Describe,
            Job::Classify,
            Job::Sort,
            Job::Summarise,
            Job::Extract,
        ] {
            assert_eq!(
                j.tier(),
                Tier::Small,
                "{} went to the large model",
                j.as_str()
            );
        }
    }

    #[test]
    fn the_thinking_goes_to_the_large_one() {
        // A bad plan is shown to a person who may say yes to it, and prose a
        // person reads as an answer had better read like one.
        assert_eq!(Job::Plan.tier(), Tier::Large);
        assert_eq!(Job::Converse.tier(), Tier::Large);
    }

    #[test]
    fn no_model_decides_whether_something_is_safe() {
        // Not the small one and not the large one. Risk comes from a table
        // that can be read; a judgement cannot be, and one that varies with
        // how the question was asked is exactly what an attacker phrases
        // carefully.
        assert!(!may_ask_a_model(Decision::WhetherItIsSafe));
        assert!(!may_ask_a_model(Decision::WhetherToAllowIt));
        // While the things a model is good at remain its to answer.
        assert!(may_ask_a_model(Decision::WhatToDo));
        assert!(may_ask_a_model(Decision::HowToSayIt));
        assert!(may_ask_a_model(Decision::WhereItBelongs));
    }

    #[test]
    fn a_small_job_is_given_a_small_budget() {
        // A model handed a large budget will use it, and a six-word
        // description arriving as four paragraphs has cost more than the
        // smaller model saved.
        assert!(Job::Classify.max_tokens() <= 16);
        assert!(Job::Describe.max_tokens() < Job::Converse.max_tokens());
        assert!(Job::Converse.max_tokens() < Job::Plan.max_tokens());
        for j in [Job::Describe, Job::Classify, Job::Sort, Job::Extract] {
            assert!(
                j.max_tokens() < Job::Plan.max_tokens() / 4,
                "{} is budgeted like a planning job",
                j.as_str()
            );
        }
    }

    #[test]
    fn a_machine_with_only_a_large_model_can_still_do_the_small_work() {
        // The tier is about spending the least that will do, not about
        // refusing to work.
        for j in [
            Job::Plan,
            Job::Converse,
            Job::Describe,
            Job::Classify,
            Job::Sort,
            Job::Summarise,
            Job::Extract,
        ] {
            assert!(
                j.may_escalate(),
                "{} cannot run without a small model",
                j.as_str()
            );
        }
    }

    #[test]
    fn a_job_can_be_named_by_a_caller_and_reads_back_the_same() {
        for j in [
            Job::Plan,
            Job::Converse,
            Job::Describe,
            Job::Classify,
            Job::Sort,
            Job::Summarise,
            Job::Extract,
        ] {
            assert_eq!(
                Job::named(j.as_str()),
                Some(j),
                "{} does not round-trip",
                j.as_str()
            );
        }
        assert_eq!(
            Job::named("SUMMARIZE"),
            Some(Job::Summarise),
            "spelling should not matter"
        );
        assert_eq!(Job::named("nonsense"), None);
    }

    #[test]
    fn every_job_has_a_name_for_the_log() {
        // A router nobody can see the decisions of is one nobody can tune.
        let mut seen: Vec<&str> = Vec::new();
        for j in [
            Job::Plan,
            Job::Converse,
            Job::Describe,
            Job::Classify,
            Job::Sort,
            Job::Summarise,
            Job::Extract,
        ] {
            assert!(!j.as_str().is_empty());
            assert!(
                !seen.contains(&j.as_str()),
                "two jobs called {}",
                j.as_str()
            );
            seen.push(j.as_str());
        }
    }
}
