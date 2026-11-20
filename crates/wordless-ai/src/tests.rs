//! Ported tests: v1 `src/ai/providers/providers.test.ts` and
//! `src/modules/ai/ai.test.ts` (Milestone 6), as Rust tests.
//!
//! Mode enforcement is the heart of this milestone, so it is tested
//! end-to-end through `run_task` with the mock provider
//! (deterministic): supervised creates an approval and applies
//! nothing; manual creates neither approval nor application;
//! autonomous applies the low-risk draft and still gates high-risk
//! actions behind an approval. Live Workers AI calls are never
//! observed here — the Workers AI tests use a stub `AiBackend`
//! (the same trait the native `worker::Ai` binding implements on
//! wasm32) or assert the honest "not configured" behaviour.

use std::rc::Rc;

use async_trait::async_trait;
use wordless_kernel::error::WordlessError;

use crate::agents::{get_agent, list_agents, AGENTS};
use crate::apply::apply_proposal;
use crate::env::AiEnv;
use crate::memory::{
    add_memory_note, build_memory_context, delete_memory_note, list_memory, rebuild_memory,
};
use crate::orchestrator::{create_task, get_task, list_tasks, process_proposals, run_task};
use crate::providers::backend::{AiBackend, AiChatOutput, ChatMessage, HttpClient};
use crate::providers::mock::MockProvider;
use crate::providers::openai::OpenAiProvider;
use crate::providers::types::{CompleteOptions, Provider, ProviderId, ProviderUsage};
use crate::providers::workers_ai::WorkersAiProvider;
use crate::providers::{get_selected_provider_id, list_providers, select_provider, test_provider};
use crate::service::{
    approve_approval, create_approval, decide_approval, get_ai_mode, list_approvals,
    reject_approval, set_ai_mode,
};
use crate::test_support::SqliteDb;
use crate::types::{AiMode, CreateAiTaskInput, ProposalActionType, ProposalRisk, ProposedAction};
use futures_executor::block_on;

/// A stub Workers AI backend: the same `AiBackend` trait the native
/// `worker::Ai` binding implements on wasm32.
struct StubBackend {
    text: String,
}

#[async_trait(?Send)]
impl AiBackend for StubBackend {
    async fn run_chat(
        &self,
        _model: &str,
        _messages: &[ChatMessage],
        _max_tokens: u32,
        _temperature: f32,
    ) -> Result<AiChatOutput, WordlessError> {
        Ok(AiChatOutput {
            response: self.text.clone(),
            usage: ProviderUsage::default(),
        })
    }
}

/// A stub HTTP client for the OpenAI-compatible provider.
struct StubHttp {
    status: u16,
    body: String,
}

#[async_trait(?Send)]
impl HttpClient for StubHttp {
    async fn post_json(
        &self,
        _url: &str,
        _bearer: &str,
        _body: &serde_json::Value,
    ) -> Result<(u16, String), WordlessError> {
        Ok((self.status, self.body.clone()))
    }
}

async fn use_mock(db: &SqliteDb) {
    select_provider(db, "mock").await.expect("select mock");
}

async fn run_mock_task(
    db: &SqliteDb,
    agent: &str,
    input: &str,
    mode: Option<AiMode>,
) -> crate::types::AiTaskView {
    use_mock(db).await;
    let created = create_task(
        db,
        CreateAiTaskInput {
            agent: agent.to_string(),
            input: input.to_string(),
            mode,
        },
    )
    .await
    .expect("create task");
    run_task(db, 1, &created.id, &AiEnv::empty(), 1)
        .await
        .expect("run task")
}

async fn post_count(db: &SqliteDb) -> i64 {
    wordless_cms::posts::list_posts(
        db,
        1,
        wordless_cms::posts::ListPostsOptions {
            status: None,
            post_type: None,
            author_id: None,
            page: Some(1),
            per_page: Some(10),
            include_trash: false,
        },
    )
    .await
    .expect("list posts")
    .total
}

#[test]
fn mock_provider_is_deterministic() {
    block_on(async {
        let env = AiEnv::empty();
        let a = MockProvider
            .complete(&env, "Write about cats", &CompleteOptions::new())
            .await
            .unwrap();
        let b = MockProvider
            .complete(&env, "Write about cats", &CompleteOptions::new())
            .await
            .unwrap();
        assert_eq!(a.text, b.text);
        assert_eq!(a.model, "mock-1.0");
        let c = MockProvider
            .complete(&env, "Write about dogs", &CompleteOptions::new())
            .await
            .unwrap();
        assert_ne!(a.text, c.text);
    });
}

#[test]
fn mock_provider_embeds_deterministically_with_8_dimensions() {
    block_on(async {
        let env = AiEnv::empty();
        let a = MockProvider.embed(&env, "hello").await.unwrap();
        let b = MockProvider.embed(&env, "hello").await.unwrap();
        assert_eq!(a.len(), 8);
        assert_eq!(a, b);
    });
}

#[test]
fn mock_provider_is_always_configured_and_probe_passes() {
    block_on(async {
        let env = AiEnv::empty();
        assert!(MockProvider.is_configured(&env));
        assert!(MockProvider.test(&env).await.ok);
    });
}

#[test]
fn workers_ai_reports_not_configured_when_binding_absent() {
    block_on(async {
        let env = AiEnv::empty();
        assert!(!WorkersAiProvider.is_configured(&env));
        assert!(WorkersAiProvider
            .configured_detail(&env)
            .contains("not present"));
        let err = WorkersAiProvider
            .complete(&env, "hi", &CompleteOptions::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, "ai_not_configured");
        let probe = WorkersAiProvider.test(&env).await;
        assert!(!probe.ok);
        assert!(probe.message.contains("not configured"));
    });
}

#[test]
fn workers_ai_reports_configured_with_backend_and_completes() {
    block_on(async {
        let env = AiEnv::empty().with_workers_ai(Rc::new(StubBackend {
            text: "ok".to_string(),
        }));
        assert!(WorkersAiProvider.is_configured(&env));
        let result = WorkersAiProvider
            .complete(&env, "hi", &CompleteOptions::new())
            .await
            .unwrap();
        assert_eq!(result.text, "ok");
        assert_eq!(result.model, "@cf/meta/llama-3-8b-instruct");
        let probe = WorkersAiProvider.test(&env).await;
        assert!(probe.ok);
    });
}

#[test]
fn openai_reports_not_configured_without_key_and_configured_with_key() {
    block_on(async {
        let env = AiEnv::empty();
        assert!(!OpenAiProvider.is_configured(&env));
        assert!(OpenAiProvider.requires_key());
        let err = OpenAiProvider
            .complete(&env, "hi", &CompleteOptions::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, "ai_not_configured");
        let probe = OpenAiProvider.test(&env).await;
        assert!(!probe.ok);

        let keyed = AiEnv::empty().with_openai_key("sk-test");
        assert!(OpenAiProvider.is_configured(&keyed));
    });
}

#[test]
fn openai_completes_through_http_client_and_parses_usage() {
    block_on(async {
        let body = r#"{"choices":[{"message":{"content":"hello from stub"}}],"usage":{"prompt_tokens":3,"completion_tokens":5,"total_tokens":8}}"#;
        let env = AiEnv::empty()
            .with_openai_key("sk-test")
            .with_http(Rc::new(StubHttp {
                status: 200,
                body: body.to_string(),
            }));
        let result = OpenAiProvider
            .complete(&env, "hi", &CompleteOptions::new())
            .await
            .unwrap();
        assert_eq!(result.text, "hello from stub");
        assert_eq!(result.model, "gpt-4o-mini");
        assert_eq!(result.usage.prompt_tokens, Some(3));
        assert_eq!(result.usage.total_tokens, Some(8));
    });
}

#[test]
fn openai_reports_http_status_without_leaking_key() {
    block_on(async {
        let env = AiEnv::empty()
            .with_openai_key("sk-test")
            .with_http(Rc::new(StubHttp {
                status: 401,
                body: "unauthorized".to_string(),
            }));
        let err = OpenAiProvider
            .complete(&env, "hi", &CompleteOptions::new())
            .await
            .unwrap_err();
        assert_eq!(err.code, "ai_provider_error");
        assert!(err.message.contains("401"));
        assert!(!err.message.contains("sk-test"));
    });
}

#[test]
fn provider_registry_lists_three_and_defaults_to_workers_ai() {
    block_on(async {
        let env = AiEnv::empty();
        let list = list_providers(&env);
        assert_eq!(
            list.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["workers-ai", "openai", "mock"]
        );
        assert!(
            list.iter()
                .find(|p| p.id == ProviderId::WorkersAi)
                .unwrap()
                .recommended
        );
        assert!(
            !list
                .iter()
                .find(|p| p.id == ProviderId::WorkersAi)
                .unwrap()
                .configured
        );
        assert!(
            list.iter()
                .find(|p| p.id == ProviderId::Mock)
                .unwrap()
                .configured
        );

        let db = SqliteDb::new();
        assert_eq!(
            get_selected_provider_id(&db).await.unwrap(),
            ProviderId::WorkersAi
        );
        let probe = test_provider(&env, "mock").await.unwrap();
        assert!(probe.ok);
        assert!(!test_provider(&env, "workers-ai").await.unwrap().ok);
        assert!(test_provider(&env, "anthropic").await.is_err());
    });
}

#[test]
fn operating_mode_defaults_supervised_and_round_trips() {
    block_on(async {
        let db = SqliteDb::new();
        assert_eq!(get_ai_mode(&db).await.unwrap(), AiMode::Supervised);
        assert_eq!(
            set_ai_mode(&db, "autonomous").await.unwrap(),
            AiMode::Autonomous
        );
        assert_eq!(get_ai_mode(&db).await.unwrap(), AiMode::Autonomous);
        assert!(set_ai_mode(&db, "chaos").await.is_err());
    });
}

#[test]
fn supervised_task_creates_approval_and_approval_creates_exactly_one_draft() {
    block_on(async {
        let db = SqliteDb::new();
        let finished = run_mock_task(&db, "content-writer", "Write about test topic", None).await;
        assert_eq!(finished.mode, AiMode::Supervised);
        assert_eq!(finished.status, crate::types::AiTaskStatus::Done);
        let result = finished.result.expect("task result");
        assert_eq!(result.proposals.len(), 1);
        assert_eq!(result.approvals.len(), 1);
        assert_eq!(result.applied.len(), 0);
        // Nothing was created yet.
        assert_eq!(post_count(&db).await, 0);
        // The approval is pending.
        let pending = list_approvals(&db, Some("pending"), 1, 20).await.unwrap();
        assert_eq!(pending.total, 1);
        // Approving applies it through the posts service: a real draft post.
        let decided = approve_approval(&db, 1, &pending.approvals[0].id, 1)
            .await
            .unwrap();
        assert_eq!(decided.status, crate::types::ApprovalStatus::Approved);
        let posts = wordless_cms::posts::list_posts(
            &db, 1,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(10),
                include_trash: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(posts.total, 1);
        assert_eq!(posts.posts[0].status.as_str(), "draft");
        assert!(posts.posts[0].title.contains("test topic"));
    });
}

#[test]
fn rejection_records_reason_and_deciding_twice_errors() {
    block_on(async {
        let db = SqliteDb::new();
        run_mock_task(&db, "content-writer", "Write about rejection", None).await;
        let pending = list_approvals(&db, Some("pending"), 1, 20).await.unwrap();
        let decided = reject_approval(&db, 1, &pending.approvals[0].id, 1, Some("Not our tone."))
            .await
            .unwrap();
        assert_eq!(decided.status, crate::types::ApprovalStatus::Rejected);
        assert_eq!(decided.reason.as_deref(), Some("Not our tone."));
        assert_eq!(post_count(&db).await, 0);
        // Deciding twice is an error.
        let err = decide_approval(&db, 1, &pending.approvals[0].id, true, 1, None)
            .await
            .unwrap_err();
        assert!(err.message.contains("already been rejected"));
    });
}

#[test]
fn manual_mode_never_creates_approvals_or_applies_anything() {
    block_on(async {
        let db = SqliteDb::new();
        let finished = run_mock_task(
            &db,
            "content-writer",
            "Write about manual mode",
            Some(AiMode::Manual),
        )
        .await;
        assert_eq!(finished.status, crate::types::AiTaskStatus::Done);
        let result = finished.result.expect("task result");
        assert_eq!(result.proposals.len(), 1);
        assert_eq!(result.approvals.len(), 0);
        assert_eq!(result.applied.len(), 0);
        assert_eq!(list_approvals(&db, None, 1, 20).await.unwrap().total, 0);
        assert_eq!(post_count(&db).await, 0);
    });
}

#[test]
fn autonomous_mode_auto_applies_low_risk_and_still_gates_publish() {
    block_on(async {
        let db = SqliteDb::new();
        let finished = run_mock_task(
            &db,
            "content-writer",
            "Write about autonomous mode",
            Some(AiMode::Autonomous),
        )
        .await;
        assert_eq!(finished.status, crate::types::AiTaskStatus::Done);
        let result = finished.result.expect("task result");
        // The low-risk draft was applied immediately, no approval created.
        assert_eq!(result.applied.len(), 1);
        assert_eq!(result.approvals.len(), 0);
        assert_eq!(post_count(&db).await, 1);

        // A high-risk proposal (publishing) still becomes an approval.
        let posts = wordless_cms::posts::list_posts(
            &db, 1,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(10),
                include_trash: false,
            },
        )
        .await
        .unwrap();
        let post_id = posts.posts[0].id;
        let handled = process_proposals(
            &db, 1,
            AiMode::Autonomous,
            &[ProposedAction::new(
                ProposalActionType::PublishPost,
                format!("Publish post {post_id}"),
                ProposalRisk::High,
                serde_json::json!({"postId": post_id}),
            )],
            &finished.id,
            "content-writer",
            1,
        )
        .await
        .unwrap();
        assert_eq!(handled.applied.len(), 0);
        assert_eq!(handled.approvals.len(), 1);
        // The post is still a draft.
        let posts = wordless_cms::posts::list_posts(
            &db, 1,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(10),
                include_trash: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(posts.posts[0].status.as_str(), "draft");
    });
}

#[test]
fn task_fails_honestly_when_no_provider_configured() {
    block_on(async {
        let db = SqliteDb::new();
        // Default provider is workers-ai; the test env has no AI binding.
        let created = create_task(
            &db,
            CreateAiTaskInput {
                agent: "content-writer".to_string(),
                input: "Write about gating".to_string(),
                mode: None,
            },
        )
        .await
        .unwrap();
        let finished = run_task(&db, 1, &created.id, &AiEnv::empty(), 1)
            .await
            .unwrap();
        assert_eq!(finished.status, crate::types::AiTaskStatus::Failed);
        assert!(finished
            .error
            .expect("error")
            .contains("No AI provider is configured"));
    });
}

#[test]
fn deterministic_seo_auditor_runs_without_any_provider() {
    block_on(async {
        let db = SqliteDb::new();
        db.insert_post(
            "No excerpt here",
            "Body.",
            "",
            "publish",
            "no-excerpt-here",
            "post",
            "2026-11-19T00:00:00.000Z",
            1,
        )
        .await;
        let finished = run_mock_task(&db, "seo-auditor", "Audit the site", None).await;
        assert_eq!(finished.status, crate::types::AiTaskStatus::Done);
        let data = finished.result.expect("result").data.expect("data");
        assert_eq!(data.get("missingExcerpt").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(data.get("total").and_then(|v| v.as_i64()), Some(1));
    });
}

#[test]
fn agent_roster_has_exactly_15_agents() {
    assert_eq!(AGENTS.len(), 15);
    let ids: Vec<&str> = AGENTS.iter().map(|a| a.id()).collect();
    let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
    assert_eq!(unique.len(), 15);
    for agent in AGENTS {
        assert!(!agent.name().is_empty());
        assert!(!agent.description().is_empty());
        assert!(!agent.capabilities().is_empty());
    }
    assert!(ids.contains(&"commerce-assistant"));
    assert!(ids.contains(&"content-writer"));
    assert!(ids.contains(&"research-assistant"));
    assert!(get_agent("no-such-agent").is_none());
}

#[test]
fn commerce_assistant_runs_on_empty_store_and_reports_zeros() {
    block_on(async {
        let db = SqliteDb::new();
        let finished =
            run_mock_task(&db, "commerce-assistant", "How is the store doing?", None).await;
        assert_eq!(finished.status, crate::types::AiTaskStatus::Done);
        let data = finished.result.expect("result").data.expect("data");
        assert_eq!(data.get("products").and_then(|v| v.as_i64()), Some(0));
    });
}

#[test]
fn unknown_agent_tasks_are_rejected() {
    block_on(async {
        let db = SqliteDb::new();
        let err = create_task(
            &db,
            CreateAiTaskInput {
                agent: "no-such-agent".to_string(),
                input: "x".to_string(),
                mode: None,
            },
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("Unknown AI agent"));
    });
}

#[test]
fn provider_agents_unavailable_without_provider_deterministic_ones_available() {
    let listed = list_agents(false);
    let writer = listed.iter().find(|a| a.id == "content-writer").unwrap();
    assert!(!writer.available);
    assert!(writer
        .unavailable_reason
        .as_deref()
        .unwrap()
        .contains("No AI provider is configured"));
    let seo = listed.iter().find(|a| a.id == "seo-auditor").unwrap();
    assert!(seo.available);
    let listed = list_agents(true);
    assert!(
        listed
            .iter()
            .find(|a| a.id == "content-writer")
            .unwrap()
            .available
    );
}

#[test]
fn website_memory_rebuild_ingests_posts_and_notes_flow() {
    block_on(async {
        let db = SqliteDb::new();
        db.set_option_raw("blogname", "\"Test Site\"").await;
        wordless_cms::posts::create_post(
            &db, 1,
            wordless_cms::posts::CreatePostInput {
                title: "Memory test post".to_string(),
                content: Some("Body.".to_string()),
                excerpt: None,
                slug: None,
                status: Some("publish".to_string()),
                post_type: Some("post".to_string()),
                author_id: 1,
                parent_id: None,
                mime_type: None,
                featured_image_id: None,
            },
        )
        .await
        .unwrap();
        let rebuilt = rebuild_memory(&db, 1).await.unwrap();
        assert!(rebuilt.facts > 0);
        let note = add_memory_note(&db, "Our tone is plain and friendly.")
            .await
            .unwrap();
        assert_eq!(note.kind, "note");
        let context = build_memory_context(&db).await.unwrap();
        assert!(context.contains("Site title: Test Site"));
        assert!(context.contains("Memory test post"));
        assert!(context.contains("Our tone is plain and friendly."));
        assert!(delete_memory_note(&db, note.id).await.unwrap());
        let after = build_memory_context(&db).await.unwrap();
        assert!(!after.contains("Our tone is plain and friendly."));
    });
}

#[test]
fn memory_refuses_secrets_and_refuses_deleting_facts() {
    block_on(async {
        let db = SqliteDb::new();
        rebuild_memory(&db, 1).await.unwrap();
        let err = add_memory_note(&db, "password: hunter2").await.unwrap_err();
        assert!(err.message.contains("must not contain"));
        let err = add_memory_note(&db, "sk-abc1234567890").await.unwrap_err();
        assert!(err.message.contains("must not contain"));
        let list = list_memory(&db).await.unwrap();
        let fact = list.facts.first().expect("a fact");
        let err = delete_memory_note(&db, fact.id).await.unwrap_err();
        assert!(err.message.contains("Only admin notes"));
    });
}

#[test]
fn task_views_round_trip_and_404_for_unknown_ids() {
    block_on(async {
        let db = SqliteDb::new();
        use_mock(&db).await;
        let created = create_task(
            &db,
            CreateAiTaskInput {
                agent: "seo-auditor".to_string(),
                input: "Audit".to_string(),
                mode: None,
            },
        )
        .await
        .unwrap();
        let fetched = get_task(&db, &created.id).await.unwrap();
        assert_eq!(fetched.agent, "seo-auditor");
        assert!(get_task(&db, "no-such-id").await.is_err());
        let listed = list_tasks(&db, 1, 20).await.unwrap();
        assert_eq!(listed.total, 1);
    });
}

#[test]
fn create_post_proposal_always_creates_a_draft_even_when_payload_says_publish() {
    block_on(async {
        let db = SqliteDb::new();
        let result = apply_proposal(
            &db, 1,
            &ProposedAction::new(
                ProposalActionType::CreatePost,
                "Create draft post",
                ProposalRisk::Low,
                serde_json::json!({"title": "Sneaky publish", "content": "Body.", "status": "publish"}),
            ),
            1,
        )
        .await
        .unwrap();
        assert!(result.get("postId").is_some());
        let posts = wordless_cms::posts::list_posts(
            &db, 1,
            wordless_cms::posts::ListPostsOptions {
                status: None,
                post_type: None,
                author_id: None,
                page: Some(1),
                per_page: Some(10),
                include_trash: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(posts.total, 1);
        assert_eq!(posts.posts[0].status.as_str(), "draft");
    });
}

#[test]
fn there_is_no_delete_action_type() {
    assert!(crate::types::ProposalActionType::from_str("delete_post").is_none());
    assert!(crate::types::ProposalActionType::from_str("delete_term").is_none());
    assert_eq!(
        crate::types::ProposalActionType::from_str("create_post"),
        Some(ProposalActionType::CreatePost)
    );
}

#[test]
fn update_option_allowlist_permits_blogname_and_rejects_other_keys() {
    block_on(async {
        let db = SqliteDb::new();
        let result = apply_proposal(
            &db, 1,
            &ProposedAction::new(
                ProposalActionType::UpdateOption,
                "Set blogname",
                ProposalRisk::Medium,
                serde_json::json!({"key": "blogname", "value": "New Name"}),
            ),
            1,
        )
        .await
        .unwrap();
        assert_eq!(result.get("key").and_then(|v| v.as_str()), Some("blogname"));
        let err = apply_proposal(
            &db, 1,
            &ProposedAction::new(
                ProposalActionType::UpdateOption,
                "Set admin email",
                ProposalRisk::High,
                serde_json::json!({"key": "admin_email", "value": "x@example.test"}),
            ),
            1,
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("not on the AI allow-list"));
    });
}

#[test]
fn approving_a_publish_approval_publishes_the_post() {
    block_on(async {
        let db = SqliteDb::new();
        let post = wordless_cms::posts::create_post(
            &db, 1,
            wordless_cms::posts::CreatePostInput {
                title: "To publish".to_string(),
                content: Some("Body.".to_string()),
                excerpt: None,
                slug: None,
                status: Some("draft".to_string()),
                post_type: Some("post".to_string()),
                author_id: 1,
                parent_id: None,
                mime_type: None,
                featured_image_id: None,
            },
        )
        .await
        .unwrap();
        let approval = create_approval(
            &db,
            "test-task",
            "content-writer",
            &ProposedAction::new(
                ProposalActionType::PublishPost,
                format!("Publish post {}", post.id),
                ProposalRisk::High,
                serde_json::json!({"postId": post.id}),
            ),
            1,
        )
        .await
        .unwrap();
        let decided = approve_approval(&db, 1, &approval.id, 1).await.unwrap();
        assert_eq!(decided.status, crate::types::ApprovalStatus::Approved);
        let fetched = wordless_cms::posts::get_post_by_id(&db, 1, post.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.status.as_str(), "publish");
    });
}

#[test]
fn deterministic_agents_run_on_an_empty_site_without_a_provider() {
    block_on(async {
        let db = SqliteDb::new();
        for agent in [
            "security-scanner",
            "performance-analyst",
            "accessibility-checker",
            "analytics-reporter",
            "backup-advisor",
            "migration-assistant",
            "code-reviewer",
        ] {
            let finished = run_mock_task(&db, agent, "Check", None).await;
            assert_eq!(
                finished.status,
                crate::types::AiTaskStatus::Done,
                "agent {agent} failed"
            );
        }
        // The security scan on a site with one admin user (no 2FA, default login).
        let admin_id = db.insert_user("admin", "admin@example.test", "Admin").await;
        db.set_user_meta_raw(admin_id, "wp_capabilities", r#"{"administrator": true}"#)
            .await;
        let finished = run_mock_task(&db, "security-scanner", "Scan", None).await;
        let data = finished.result.expect("result").data.expect("data");
        assert_eq!(data.get("users").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(data.get("administrators").and_then(|v| v.as_i64()), Some(1));
        assert_eq!(
            data.get("administratorsWith2fa").and_then(|v| v.as_i64()),
            Some(0)
        );
        assert_eq!(
            data.get("defaultAdminLoginExists")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
    });
}
