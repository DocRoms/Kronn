use super::*;

use crate::acp::{
    AcpAgent, AcpCapability, AcpConfigOption, AcpError, AcpInitialize, AcpNegotiatedCapabilities,
    AcpSessionEvent, AcpSessionTarget, AcpTransport,
};

use std::collections::BTreeSet;

use std::sync::atomic::{AtomicUsize, Ordering};

enum PromptOutcome {
    Complete,
    Fail(String),
    WaitForCancel,
}

#[derive(Clone, Copy)]
enum ResumeOutcome {
    Ok,
    Missing,
    CapabilityUnavailable,
    Auth,
    Transport,
    Timeout,
}

struct RunnerTransport {
    agent: AcpAgent,
    resume_outcome: ResumeOutcome,
    prompt_outcome: PromptOutcome,
    native_session_id: Option<String>,
    shutdown_error: Option<String>,
    created: AtomicUsize,
    resumed: AtomicUsize,
    cancelled: AtomicUsize,
    prompts: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl AcpTransport for RunnerTransport {
    async fn initialize(&self, _: AcpInitialize) -> Result<AcpNegotiatedCapabilities, AcpError> {
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: BTreeSet::from([
                AcpCapability::Sessions,
                AcpCapability::Resume,
                AcpCapability::Streaming,
                AcpCapability::Cancellation,
                AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        self.created.fetch_add(1, Ordering::SeqCst);
        AcpSessionTarget::new(self.agent, "fresh-session")
    }
    async fn config_options(&self) -> Vec<AcpConfigOption> {
        Vec::new()
    }
    async fn set_config_option(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        _: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }
    async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        self.resumed.fetch_add(1, Ordering::SeqCst);
        match self.resume_outcome {
            ResumeOutcome::Ok => Ok(()),
            ResumeOutcome::Missing => Err(AcpError::SessionNotFound),
            ResumeOutcome::CapabilityUnavailable => Err(AcpError::CapabilityUnavailable {
                capability: AcpCapability::Resume,
            }),
            ResumeOutcome::Auth => Err(AcpError::Transport("authentication refused".into())),
            ResumeOutcome::Transport => Err(AcpError::Transport("network unavailable".into())),
            ResumeOutcome::Timeout => Err(AcpError::Timeout("session/resume".into())),
        }
    }
    async fn prompt(
        &self,
        _: &AcpSessionTarget,
        prompt: &str,
        events: tokio::sync::mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.prompts.lock().unwrap().push(prompt.to_owned());
        match &self.prompt_outcome {
            PromptOutcome::Complete => {
                events
                    .send(AcpSessionEvent::TextDelta("first chunk".into()))
                    .await
                    .unwrap();
                events
                    .send(AcpSessionEvent::ToolCall {
                        id: None,
                        name: "read_file".into(),
                    })
                    .await
                    .unwrap();
                events
                    .send(AcpSessionEvent::Usage {
                        input_tokens: 3,
                        output_tokens: 5,
                        prompt_cache: Default::default(),
                    })
                    .await
                    .unwrap();
                if let Some(session_id) = &self.native_session_id {
                    events
                        .send(AcpSessionEvent::NativeSessionId(session_id.clone()))
                        .await
                        .unwrap();
                }
                events.send(AcpSessionEvent::Completed).await.unwrap();
                Ok(())
            }
            PromptOutcome::Fail(error) => Err(AcpError::Transport(error.clone())),
            PromptOutcome::WaitForCancel => std::future::pending().await,
        }
    }
    async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        self.cancelled.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), AcpError> {
        match &self.shutdown_error {
            Some(error) => Err(AcpError::Transport(error.clone())),
            None => Ok(()),
        }
    }
}

fn transport(resume_outcome: ResumeOutcome, prompt_outcome: PromptOutcome) -> Arc<RunnerTransport> {
    Arc::new(RunnerTransport {
        agent: AcpAgent::OpenCode,
        resume_outcome,
        prompt_outcome,
        native_session_id: None,
        shutdown_error: None,
        created: AtomicUsize::new(0),
        resumed: AtomicUsize::new(0),
        cancelled: AtomicUsize::new(0),
        prompts: Mutex::new(Vec::new()),
    })
}

async fn collect_output(process: &mut AgentProcess) -> String {
    let mut output = String::new();
    while let Some(line) = process.next_line().await {
        output.push_str(&line);
    }
    output
}

/// KT-1278 — over ACP the context and the task are ONE user message. An
/// audit step hands the session a prompt whose last words are its own
/// instruction, and the session puts that prompt last, after the context
/// (the workflow step and the task worker brief are pinned at their own
/// assembly: `build_step_prompt`, `worker_brief_markdown`).
#[tokio::test]
async fn an_acp_message_with_context_ends_with_the_tasks_own_instruction() {
    let docs_root = crate::core::docs_location::DocsRoot::default_root();
    let step = crate::api::audit::ANALYSIS_STEPS[7];
    let prompt = crate::api::audit::finish_step_prompt(
        "Step 8 instructions\n\n### Runtime versions pinned in the repository\n\nNone found.\n",
        None,
        8,
        &step,
        &docs_root,
    );
    let context = "=== ANTI-HALLUCINATION (pointer) ===\n\nCite your sources.\n\n=== AVAILABLE TOOLS ===\n\nA table of tools.";
    let fixture = transport(ResumeOutcome::Ok, PromptOutcome::Complete);
    let mut process = run_acp_session(
        AcpSessionRequest {
            step_tools: None,
            agent_type: &AgentType::OpenCode,
            work_dir: Path::new("."),
            prompt: &prompt,
            system_context: context,
            project_path: "",
            model_flag: None,
            reasoning_effort: None,
            parent_cancel: None,
            discussion_id: None,
            resume_id: None,
            session_store: None,
            fallback_prompt: None,
            provenance: None,
            activity: None,
            idle_timeout: None,
            run_progress: None,
        },
        fixture.clone(),
    )
    .await
    .expect("run ACP fixture");
    collect_output(&mut process).await;
    let sent = fixture.prompts.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].starts_with(context), "context first");
    assert!(sent[0].ends_with(&prompt), "the task's prompt comes last");
    assert!(
        sent[0]
            .trim_end()
            .ends_with("Stop once the work is written."),
        "the message must end on the task's instruction, got …{}",
        &sent[0][sent[0].len().saturating_sub(200)..]
    );
}

/// A session that lists models the way OpenCode does and records what the
/// runner asks it to run. The model reaches the agent only through
/// `session/set_config_option`, so that call is "the model that runs".
struct ModelSessionTransport {
    options: Vec<AcpConfigOption>,
    selections: Mutex<Vec<(String, String)>>,
    prompts: AtomicUsize,
    shutdowns: AtomicUsize,
}

impl ModelSessionTransport {
    fn offering(models: &[&str]) -> Arc<Self> {
        let options = if models.is_empty() {
            Vec::new()
        } else {
            vec![AcpConfigOption {
                id: "model".into(),
                current: models.first().map(|model| (*model).to_owned()),
                available: models
                    .iter()
                    .map(|model| crate::acp::AcpConfigValue {
                        id: (*model).to_owned(),
                        name: (*model).to_owned(),
                    })
                    .collect(),
            }]
        };
        Arc::new(Self {
            options,
            selections: Mutex::new(Vec::new()),
            prompts: AtomicUsize::new(0),
            shutdowns: AtomicUsize::new(0),
        })
    }

    fn selections(&self) -> Vec<(String, String)> {
        self.selections.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl AcpTransport for ModelSessionTransport {
    async fn initialize(&self, _: AcpInitialize) -> Result<AcpNegotiatedCapabilities, AcpError> {
        Ok(AcpNegotiatedCapabilities {
            protocol_version: 1,
            capabilities: BTreeSet::from([
                AcpCapability::Sessions,
                AcpCapability::Resume,
                AcpCapability::Streaming,
                AcpCapability::Cancellation,
                AcpCapability::McpInjection,
            ]),
        })
    }
    async fn create_session(&self) -> Result<AcpSessionTarget, AcpError> {
        AcpSessionTarget::new(AcpAgent::OpenCode, "model-session")
    }
    async fn config_options(&self) -> Vec<AcpConfigOption> {
        self.options.clone()
    }
    async fn set_config_option(
        &self,
        _: &AcpSessionTarget,
        config_id: &str,
        value_id: &str,
    ) -> Result<(), AcpError> {
        self.selections
            .lock()
            .unwrap()
            .push((config_id.to_owned(), value_id.to_owned()));
        Ok(())
    }
    async fn resume_session(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }
    async fn prompt(
        &self,
        _: &AcpSessionTarget,
        _: &str,
        events: tokio::sync::mpsc::Sender<AcpSessionEvent>,
    ) -> Result<(), AcpError> {
        self.prompts.fetch_add(1, Ordering::SeqCst);
        events.send(AcpSessionEvent::Completed).await.unwrap();
        Ok(())
    }
    async fn cancel(&self, _: &AcpSessionTarget) -> Result<(), AcpError> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<(), AcpError> {
        self.shutdowns.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
