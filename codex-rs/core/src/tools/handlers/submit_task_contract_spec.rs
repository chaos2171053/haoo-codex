use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolSpec;
use serde::Deserialize;
use serde::Serialize;
use std::collections::BTreeMap;

pub const SUBMIT_TASK_CONTRACT_TOOL_NAME: &str = "submit_task_contract";

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct SubmitTaskContractArgs {
    pub result: String,
    pub boundary: String,
    pub completion: String,
    pub evidence: Vec<String>,
}

pub fn create_submit_task_contract_tool() -> ToolSpec {
    let properties = BTreeMap::from([
        (
            "result".to_string(),
            JsonSchema::string(Some("The result the user wants to obtain.".to_string())),
        ),
        (
            "boundary".to_string(),
            JsonSchema::string(Some("What this task will cover.".to_string())),
        ),
        (
            "completion".to_string(),
            JsonSchema::string(Some(
                "The observable conditions for completion.".to_string(),
            )),
        ),
        (
            "evidence".to_string(),
            JsonSchema::array(
                JsonSchema::string(None),
                Some(
                    "Short excerpts from the user's messages or explicit answers that support the result, boundary, and completion conditions."
                        .to_string(),
                ),
            ),
        ),
    ]);

    ToolSpec::Function(ResponsesApiTool {
        name: SUBMIT_TASK_CONTRACT_TOOL_NAME.to_string(),
        description: "Submit a candidate task contract for independent review before using work tools. Do not infer preferences, scope, or completion conditions that the user has not established. Ask the user when a remaining unknown would change any contract field."
            .to_string(),
        strict: false,
        defer_loading: None,
        parameters: JsonSchema::object(
            properties,
            Some(vec![
                "result".to_string(),
                "boundary".to_string(),
                "completion".to_string(),
                "evidence".to_string(),
            ]),
            Some(false.into()),
        ),
        output_schema: None,
    })
}

pub(crate) fn validate_submit_task_contract_args(
    args: SubmitTaskContractArgs,
) -> Result<SubmitTaskContractArgs, String> {
    if args.result.trim().is_empty()
        || args.boundary.trim().is_empty()
        || args.completion.trim().is_empty()
        || args
            .evidence
            .iter()
            .all(|evidence| evidence.trim().is_empty())
    {
        return Err(
            "submit_task_contract requires non-empty result, boundary, completion, and evidence"
                .to_string(),
        );
    }
    Ok(args)
}
