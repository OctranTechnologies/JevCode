//! Task plans. A plan is the answer a read-only Plan task gives, kept as its own
//! artifact with edits, a bounded version history, and a Markdown export. A plan
//! reaches implementation only through an explicit handoff to a new Agent task.
//! That task gets fresh permission state, and none of the Plan task's pending
//! calls, approvals, grants, or write authority carry over. Creating or editing
//! a plan grants nothing.
use crate::{
    domain::{AgentSession, MessageRole, TaskMode},
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};

pub const MAX_PLAN_CHARS: usize = 60_000;
pub const MAX_VERSIONS: usize = 20;
pub const MAX_COMMENTS: usize = 50;
pub const MAX_COMMENT_CHARS: usize = 2_000;
pub const MAX_ANCHOR_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanVersion {
    pub version: u32,
    pub markdown: String,
    pub saved_at: String,
}

/// A note on a plan for its author to address. Notes change no plan text and grant
/// nothing. Open notes travel with the plan's export and its handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanComment {
    pub id: String,
    pub body: String,
    /// A short quote of the plan line the note is about, when one was given.
    pub anchor: Option<String>,
    pub resolved: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskPlan {
    pub id: String,
    pub project_id: String,
    pub source_session_id: String,
    pub title: String,
    pub markdown: String,
    pub version: u32,
    /// Earlier versions, oldest first. At most `MAX_VERSIONS` are kept.
    pub history: Vec<PlanVersion>,
    #[serde(default)]
    pub comments: Vec<PlanComment>,
    pub created_at: String,
    pub updated_at: String,
}

/// Saves the Plan task's final answer as a plan. Only a Plan task can produce one.
pub fn from_task(session: &AgentSession, id: String, now: &str) -> AppResult<TaskPlan> {
    if session.task_mode != TaskMode::Plan {
        return Err(invalid(
            "Only a Plan task produces a plan. Start a Plan task to create one.",
        ));
    }
    let markdown = session
        .messages
        .iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::Assistant
                && message.tool_calls.is_empty()
                && !message.content.trim().is_empty()
        })
        .map(|message| message.content.trim().to_owned())
        .ok_or_else(|| {
            invalid("This Plan task has no plan yet. Wait for it to finish, then save the plan.")
        })?;
    check_length(&markdown)?;
    Ok(TaskPlan {
        id,
        project_id: session.project_id.clone(),
        source_session_id: session.id.clone(),
        title: format!("Plan: {}", session.title),
        markdown,
        version: 1,
        history: Vec::new(),
        comments: Vec::new(),
        created_at: now.to_owned(),
        updated_at: now.to_owned(),
    })
}

/// Adds a review note. It does not change the plan text, so it creates no version.
pub fn add_comment(
    plan: &mut TaskPlan,
    id: String,
    body: &str,
    anchor: Option<&str>,
    now: &str,
) -> AppResult<()> {
    let body = body.trim();
    if body.is_empty() {
        return Err(invalid("A review comment cannot be empty."));
    }
    if body.chars().count() > MAX_COMMENT_CHARS {
        return Err(invalid("A review comment is longer than 2,000 characters."));
    }
    if plan.comments.len() >= MAX_COMMENTS {
        return Err(invalid(
            "This plan already has 50 review comments. Resolve or remove some first.",
        ));
    }
    let anchor = anchor
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(MAX_ANCHOR_CHARS).collect());
    plan.comments.push(PlanComment {
        id,
        body: body.to_owned(),
        anchor,
        resolved: false,
        created_at: now.to_owned(),
    });
    Ok(())
}

pub fn set_comment_resolved(
    plan: &mut TaskPlan,
    comment_id: &str,
    resolved: bool,
) -> AppResult<()> {
    let comment = plan
        .comments
        .iter_mut()
        .find(|comment| comment.id == comment_id)
        .ok_or_else(|| invalid("That review comment no longer exists."))?;
    comment.resolved = resolved;
    Ok(())
}

pub fn remove_comment(plan: &mut TaskPlan, comment_id: &str) -> AppResult<()> {
    let before = plan.comments.len();
    plan.comments.retain(|comment| comment.id != comment_id);
    if plan.comments.len() == before {
        return Err(invalid("That review comment no longer exists."));
    }
    Ok(())
}

/// The notes still open, as Markdown list items.
fn open_notes(plan: &TaskPlan) -> Vec<String> {
    plan.comments
        .iter()
        .filter(|comment| !comment.resolved)
        .map(|comment| match &comment.anchor {
            Some(anchor) => format!("- {} (on \"{anchor}\")", comment.body),
            None => format!("- {}", comment.body),
        })
        .collect()
}

/// Replaces the plan text, keeping the previous version in the history.
pub fn revise(plan: &mut TaskPlan, markdown: &str, now: &str) -> AppResult<()> {
    let markdown = markdown.trim();
    if markdown.is_empty() {
        return Err(invalid("A plan cannot be empty."));
    }
    check_length(markdown)?;
    if markdown == plan.markdown {
        return Ok(());
    }
    plan.history.push(PlanVersion {
        version: plan.version,
        markdown: plan.markdown.clone(),
        saved_at: plan.updated_at.clone(),
    });
    if plan.history.len() > MAX_VERSIONS {
        let excess = plan.history.len() - MAX_VERSIONS;
        plan.history.drain(..excess);
    }
    plan.version += 1;
    plan.markdown = markdown.to_owned();
    plan.updated_at = now.to_owned();
    Ok(())
}

/// The Markdown file a user exports. It says what it is and grants nothing.
pub fn export_markdown(plan: &TaskPlan) -> String {
    let notes = open_notes(plan);
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!("\n\n## Open review comments\n\n{}", notes.join("\n"))
    };
    format!(
        "# {}\n\n{}{}\n\n---\nPlan version {} from task {}. Exported from Octran Code. This file grants no permissions.\n",
        plan.title, plan.markdown, notes, plan.version, plan.source_session_id
    )
}

/// The first message of the new Agent task. It cites the plan and its source, and
/// lists the review comments still open, which the task is asked to address.
pub fn handoff_prompt(plan: &TaskPlan) -> String {
    let notes = open_notes(plan);
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nAlso address these open review comments:\n{}",
            notes.join("\n")
        )
    };
    format!(
        "Carry out the approved plan below. It is plan {} (version {}), written in the Plan task {}. Follow its steps in order, and say which step you could not complete.\n\n{}{}",
        plan.id, plan.version, plan.source_session_id, plan.markdown, notes
    )
}

fn check_length(markdown: &str) -> AppResult<()> {
    if markdown.chars().count() > MAX_PLAN_CHARS {
        return Err(invalid("This plan is longer than 60,000 characters."));
    }
    Ok(())
}

fn invalid(message: &str) -> AppError {
    AppError::new("invalid_plan", message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::AgentMessage;

    fn plan_session() -> AgentSession {
        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        session.task_mode = TaskMode::Plan;
        session.messages.push(AgentMessage::text(
            MessageRole::Assistant,
            "1. Read the billing module.\n2. Add the retry test.",
        ));
        session
    }

    #[test]
    fn a_plan_is_the_plan_tasks_final_answer() {
        let plan = from_task(&plan_session(), "plan-1".into(), "2026-11-15T10:00:00Z").unwrap();

        assert_eq!(plan.version, 1);
        assert!(plan.markdown.starts_with("1. Read the billing module."));
        assert_eq!(plan.source_session_id, "test-session");
        assert!(plan.history.is_empty());
    }

    #[test]
    fn only_a_plan_task_produces_a_plan() {
        let mut session = plan_session();
        session.task_mode = TaskMode::Agent;

        let error = from_task(&session, "plan-1".into(), "now").unwrap_err();
        assert_eq!(error.code, "invalid_plan");
    }

    #[test]
    fn a_plan_task_with_no_answer_yet_has_no_plan() {
        let mut session: AgentSession =
            serde_json::from_str(include_str!("../../tests/fixtures/session.json")).unwrap();
        session.task_mode = TaskMode::Plan;
        session.messages.clear();

        assert!(from_task(&session, "plan-1".into(), "now").is_err());
    }

    #[test]
    fn revisions_keep_history_and_bound_it() {
        let mut plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        for index in 0..(MAX_VERSIONS + 5) {
            revise(
                &mut plan,
                &format!("Step set {index}."),
                &format!("t{index}"),
            )
            .unwrap();
        }

        assert_eq!(plan.version, (MAX_VERSIONS + 6) as u32);
        assert_eq!(plan.history.len(), MAX_VERSIONS);
        assert_eq!(
            plan.history.last().unwrap().markdown,
            format!("Step set {}.", MAX_VERSIONS + 3)
        );
        assert_eq!(plan.markdown, format!("Step set {}.", MAX_VERSIONS + 4));
    }

    #[test]
    fn saving_unchanged_text_or_empty_text_is_handled() {
        let mut plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        let before = plan.clone();

        revise(&mut plan, &before.markdown, "t1").unwrap();
        assert_eq!(plan, before);
        assert_eq!(
            revise(&mut plan, "   ", "t2").unwrap_err().code,
            "invalid_plan"
        );
    }

    #[test]
    fn export_and_handoff_cite_the_plan_and_its_source_without_authority() {
        let plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();

        let export = export_markdown(&plan);
        assert!(export.contains("grants no permissions"));
        assert!(export.contains("Plan version 1 from task test-session"));

        let handoff = handoff_prompt(&plan);
        assert!(handoff.contains("plan plan-1 (version 1)"));
        assert!(handoff.contains("Plan task test-session"));
        assert!(handoff.contains("Add the retry test."));
    }

    #[test]
    fn open_review_comments_travel_with_the_export_and_handoff() {
        let mut plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        add_comment(
            &mut plan,
            "c1".into(),
            "  Keep the retry under 3 attempts. ",
            Some("Add the retry test."),
            "t1",
        )
        .unwrap();
        add_comment(&mut plan, "c2".into(), "Already covered.", None, "t1").unwrap();
        set_comment_resolved(&mut plan, "c2", true).unwrap();

        let export = export_markdown(&plan);
        assert!(export.contains("## Open review comments"));
        assert!(export.contains("- Keep the retry under 3 attempts. (on \"Add the retry test.\")"));
        assert!(!export.contains("Already covered."));
        let handoff = handoff_prompt(&plan);
        assert!(handoff.contains("Also address these open review comments:"));
        assert!(handoff.contains("Keep the retry under 3 attempts."));
        assert!(!handoff.contains("Already covered."));
    }

    #[test]
    fn comments_change_no_plan_text_and_stay_bounded() {
        let mut plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        let original = plan.markdown.clone();

        assert!(add_comment(&mut plan, "c".into(), "   ", None, "t1").is_err());
        assert!(add_comment(
            &mut plan,
            "c".into(),
            &"x".repeat(MAX_COMMENT_CHARS + 1),
            None,
            "t1"
        )
        .is_err());
        for index in 0..MAX_COMMENTS {
            add_comment(&mut plan, format!("c{index}"), "note", None, "t1").unwrap();
        }
        assert!(add_comment(&mut plan, "late".into(), "note", None, "t1").is_err());
        assert_eq!(plan.version, 1);
        assert!(plan.history.is_empty());
        assert_eq!(plan.markdown, original);
    }

    #[test]
    fn a_comment_can_be_resolved_reopened_and_removed_only_if_it_exists() {
        let mut plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        add_comment(
            &mut plan,
            "c1".into(),
            "Check the rollback.",
            Some("   "),
            "t1",
        )
        .unwrap();
        assert_eq!(plan.comments[0].anchor, None);

        set_comment_resolved(&mut plan, "c1", true).unwrap();
        assert!(plan.comments[0].resolved);
        set_comment_resolved(&mut plan, "c1", false).unwrap();
        assert!(!plan.comments[0].resolved);
        assert!(set_comment_resolved(&mut plan, "missing", true).is_err());

        remove_comment(&mut plan, "c1").unwrap();
        assert!(plan.comments.is_empty());
        assert!(remove_comment(&mut plan, "c1").is_err());
    }

    #[test]
    fn plans_saved_before_comments_existed_load_without_them() {
        let plan = from_task(&plan_session(), "plan-1".into(), "t0").unwrap();
        let mut stored = serde_json::to_value(&plan).unwrap();
        stored.as_object_mut().unwrap().remove("comments");

        let loaded: TaskPlan = serde_json::from_value(stored).unwrap();

        assert!(loaded.comments.is_empty());
    }
}
