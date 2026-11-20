use super::*;

/// 422 friendliness — the test API call endpoint payload that
/// frontend `workflowsApi.testApiCall()` sends. Without
/// `prompt_template` / `agent` / `mode` defaults this test would
/// blow up with the exact error the JIRA helper agent surfaced
/// during dogfooding ("missing field `prompt_template`").
#[test]
fn test_api_call_request_accepts_minimal_step() {
    use crate::api::workflows::TestApiCallRequest;
    let json = r#"{
            "step": {
                "name": "fetch_issue",
                "step_type": { "type": "ApiCall" },
                "api_plugin_slug": "mcp-atlassian",
                "api_config_id": "cfg-1",
                "api_endpoint_path": "/rest/api/3/issue/EW-{{ticket_number}}",
                "api_method": "GET",
                "api_query": { "fields": "summary,description" },
                "api_extract": { "path": "$.fields", "fail_on_empty": false }
            },
            "project_id": "proj-1"
        }"#;
    let req: TestApiCallRequest = serde_json::from_str(json)
        .expect("TestApiCallRequest with minimal ApiCall step must deserialise");
    assert_eq!(req.project_id, "proj-1");
    assert_eq!(req.step.name, "fetch_issue");
}
