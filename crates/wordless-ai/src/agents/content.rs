//! Provider-based drafting agents — port of v1
//! `src/ai/agents/content.ts` (Milestone 6).
//!
//! These agents ask the configured provider (Workers AI by default,
//! mock in tests/local dev) to draft text. Every output is labelled
//! as AI output in the UI, and nothing is published by these agents —
//! drafts become `create_post` proposals (draft status) that follow
//! the site's operating mode (manual: never applied; supervised:
//! approval required; autonomous: low-risk draft creation
//! auto-applies).

use async_trait::async_trait;
use wordless_kernel::error::WordlessError;

use crate::agents::{Agent, AgentContext};
use crate::providers::CompleteOptions;
use crate::types::{AgentResult, ProposalActionType, ProposalRisk, ProposedAction};

fn draft_title(input: &str) -> String {
    let topic: String = input.split_whitespace().collect::<Vec<_>>().join(" ");
    let topic: String = topic.chars().take(60).collect();
    if topic.is_empty() {
        "AI draft".to_string()
    } else {
        format!("AI draft: {topic}")
    }
}

async fn complete(
    ctx: &AgentContext<'_>,
    system: &str,
    prompt: &str,
) -> Result<String, WordlessError> {
    let result = ctx
        .provider
        .complete(
            ctx.env,
            prompt,
            &CompleteOptions::new()
                .with_system(system)
                .with_max_tokens(1024)
                .with_temperature(0.7),
        )
        .await?;
    Ok(result.text)
}

fn create_post_proposal(title: &str, content: &str, summary: String) -> ProposedAction {
    ProposedAction::new(
        ProposalActionType::CreatePost,
        summary,
        ProposalRisk::Low,
        serde_json::json!({
            "title": title,
            "content": content,
            "status": "draft",
            "type": "post",
        }),
    )
}

pub struct ContentWriter;

#[async_trait(?Send)]
impl Agent for ContentWriter {
    fn id(&self) -> &'static str {
        "content-writer"
    }
    fn name(&self) -> &'static str {
        "Content Writer"
    }
    fn description(&self) -> &'static str {
        "Drafts a blog post on a topic you give it, using the site's title, tagline and published posts as context. The draft is proposed as a new draft post — never published by the agent."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Draft blog posts",
            "Use website memory as context",
            "Propose a draft post for review",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        true
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let prompt = format!(
            "{}\n\nWrite a blog post draft about: {}\nUse clear headings and a short introduction. Do not invent facts about the site that are not in the memory above.",
            ctx.memory_text, ctx.input
        );
        let text = complete(
            ctx,
            "You are a careful blog writer for a WordPress-style site. Write in plain, useful English.",
            &prompt,
        )
        .await?;
        let title = draft_title(&ctx.input);
        Ok(AgentResult {
            summary: format!(
                "Drafted a post about \"{}\" (AI draft — not published).",
                ctx.input.chars().take(80).collect::<String>()
            ),
            output: Some(text.clone()),
            data: None,
            proposals: Some(vec![create_post_proposal(
                &title,
                &text,
                format!("Create draft post \"{title}\""),
            )]),
            usage: None,
        })
    }
}

pub struct ResearchAssistant;

#[async_trait(?Send)]
impl Agent for ResearchAssistant {
    fn id(&self) -> &'static str {
        "research-assistant"
    }
    fn name(&self) -> &'static str {
        "Research Assistant"
    }
    fn description(&self) -> &'static str {
        "Researches a topic and writes a structured summary. It cannot browse the live web in M6 — it works from the provider's knowledge and the site's own memory, and says so in its output."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Summarise a topic",
            "Structure findings with headings",
            "Propose the summary as a draft post",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        true
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let prompt = format!(
            "{}\n\nResearch this topic and write a structured summary with headings: {}\nNote where you are unsure instead of inventing specifics. This assistant cannot browse the live web; say so if live data would be needed.",
            ctx.memory_text, ctx.input
        );
        let text = complete(
            ctx,
            "You are a careful research assistant. Structure your answer with headings. Never invent citations, dates or statistics.",
            &prompt,
        )
        .await?;
        let title = draft_title(&ctx.input);
        Ok(AgentResult {
            summary: format!(
                "Researched \"{}\" (AI summary — verify facts before publishing).",
                ctx.input.chars().take(80).collect::<String>()
            ),
            output: Some(text.clone()),
            data: None,
            proposals: Some(vec![create_post_proposal(
                &title,
                &text,
                format!("Create draft post \"{title}\" (research summary)"),
            )]),
            usage: None,
        })
    }
}

pub struct SupportResponder;

#[async_trait(?Send)]
impl Agent for SupportResponder {
    fn id(&self) -> &'static str {
        "support-responder"
    }
    fn name(&self) -> &'static str {
        "Support Responder"
    }
    fn description(&self) -> &'static str {
        "Drafts a reply to a visitor's message or comment. The reply is a draft for the admin to send — the agent never posts it itself."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Draft replies to visitor messages",
            "Match the site's tone from memory",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        true
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let prompt = format!(
            "{}\n\nA visitor wrote:\n\"{}\"\n\nDraft a polite, helpful reply. Do not promise anything the site cannot do (no refunds, no account changes) — say the admin will follow up where needed.",
            ctx.memory_text, ctx.input
        );
        let text = complete(
            ctx,
            "You are a helpful support writer. Be polite, brief and honest.",
            &prompt,
        )
        .await?;
        Ok(AgentResult {
            summary: "Drafted a reply to the visitor's message (draft only — not sent)."
                .to_string(),
            output: Some(text),
            data: None,
            proposals: None,
            usage: None,
        })
    }
}

pub struct EmailComposer;

#[async_trait(?Send)]
impl Agent for EmailComposer {
    fn id(&self) -> &'static str {
        "email-composer"
    }
    fn name(&self) -> &'static str {
        "Email Composer"
    }
    fn description(&self) -> &'static str {
        "Drafts an email (subject + body) on a topic you give it. Sending email arrives in Milestone 8 — this agent only drafts."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Draft email subject + body",
            "Use website memory as context",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        true
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let prompt = format!(
            "{}\n\nDraft an email about: {}\nStart with a \"Subject:\" line, then the body. Keep it short.",
            ctx.memory_text, ctx.input
        );
        let text = complete(
            ctx,
            "You are an email writer. Short subject, clear body, no hype.",
            &prompt,
        )
        .await?;
        Ok(AgentResult {
            summary: "Drafted an email (draft only — sending arrives in Milestone 8).".to_string(),
            output: Some(text),
            data: None,
            proposals: None,
            usage: None,
        })
    }
}

pub struct SocialComposer;

#[async_trait(?Send)]
impl Agent for SocialComposer {
    fn id(&self) -> &'static str {
        "social-composer"
    }
    fn name(&self) -> &'static str {
        "Social Composer"
    }
    fn description(&self) -> &'static str {
        "Drafts short social posts on a topic. Posting to social accounts arrives in Milestone 8 — this agent only drafts."
    }
    fn capabilities(&self) -> &[&'static str] {
        &[
            "Draft short social posts",
            "Keep posts within typical length limits",
        ]
    }
    fn risk(&self) -> ProposalRisk {
        ProposalRisk::Low
    }
    fn requires_provider(&self) -> bool {
        true
    }
    async fn run(&self, ctx: &AgentContext<'_>) -> Result<AgentResult, WordlessError> {
        let prompt = format!(
            "{}\n\nWrite 3 short social media posts about: {}\nNumber them 1-3. Keep each under 240 characters. No hashtags spam.",
            ctx.memory_text, ctx.input
        );
        let text = complete(
            ctx,
            "You are a social media writer. Short, plain, useful posts.",
            &prompt,
        )
        .await?;
        Ok(AgentResult {
            summary: "Drafted 3 social posts (drafts only — publishing to social accounts arrives in Milestone 8).".to_string(),
            output: Some(text),
            data: None,
            proposals: None,
            usage: None,
        })
    }
}
