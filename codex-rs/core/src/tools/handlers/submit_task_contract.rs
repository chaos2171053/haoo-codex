use crate::function_tool::FunctionCallError;
use crate::task_contract_review::TaskCandidate;
use crate::task_contract_review::TaskContractReviewDecision;
use crate::task_contract_review::audit_task_candidate;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::handlers::submit_task_contract_spec::SUBMIT_TASK_CONTRACT_TOOL_NAME;
use crate::tools::handlers::submit_task_contract_spec::SubmitTaskContractArgs;
use crate::tools::handlers::submit_task_contract_spec::create_submit_task_contract_tool;
use crate::tools::handlers::submit_task_contract_spec::validate_submit_task_contract_args;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

pub struct SubmitTaskContractHandler;

impl ToolExecutor<ToolInvocation> for SubmitTaskContractHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(SUBMIT_TASK_CONTRACT_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        create_submit_task_contract_tool()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let arguments = match &invocation.payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "{SUBMIT_TASK_CONTRACT_TOOL_NAME} handler received unsupported payload"
                    )));
                }
            };
            let args: SubmitTaskContractArgs = parse_arguments(arguments)?;
            let args = validate_submit_task_contract_args(args)
                .map_err(FunctionCallError::RespondToModel)?;
            let assessment = audit_task_candidate(
                invocation.session.clone(),
                invocation.turn.clone(),
                TaskCandidate::Contract(args.clone()),
                invocation.cancellation_token.clone(),
            )
            .await
            .map_err(FunctionCallError::RespondToModel)?;
            if assessment.decision == TaskContractReviewDecision::Clarify {
                let content = serde_json::to_string(&assessment).map_err(|err| {
                    FunctionCallError::Fatal(format!(
                        "failed to serialize task contract assessment: {err}"
                    ))
                })?;
                return Ok(boxed_tool_output(FunctionToolOutput::from_text(
                    content,
                    Some(false),
                )));
            }
            let content = serde_json::to_string(&args).map_err(|err| {
                FunctionCallError::Fatal(format!(
                    "failed to serialize {SUBMIT_TASK_CONTRACT_TOOL_NAME} response: {err}"
                ))
            })?;
            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                content,
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for SubmitTaskContractHandler {
    fn is_builtin_control_tool(&self) -> bool {
        true
    }
}
