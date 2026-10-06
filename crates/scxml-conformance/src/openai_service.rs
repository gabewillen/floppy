//! The conformance harness's outbound network: a deterministic, in-process
//! stand-in for the `OpenAI` Responses API (`POST {base}/responses`), and the
//! fake values the run holds for `env` slots.
//!
//! A namespace suite that binds the `openai` repository namespace
//! (`namespaces/openai/irp`, and `namespaces/env/irp` through the `openai`
//! element) runs every request its extension component makes through this
//! transport, which the session host reaches only after it admitted the
//! request (an origin the package declares, or one a held value supplies
//! entirely). Nothing touches the network: the stand-in answers from the
//! request alone, so every case is deterministic, and every key is a fake
//! test value.
//!
//! # Contract
//!
//! * Served URLs: exactly [`DECLARED_URL`] (the package's declared origin)
//!   and [`BASE_URL`]`/responses` (the origin [`BASE_URL`], a held value,
//!   supplies). A request to [`UNREACHABLE_BASE_URL`]`/responses` fails as
//!   an unreachable service ([`ExtensionHttpFailure::Transport`]); any other
//!   URL answers `404`.
//! * A served request must be a `POST` whose `authorization` header is
//!   `Bearer ` followed by the value of [`KEY`] or [`OTHER_KEY`] (else `401`)
//!   and whose body is a JSON object with a string `model` and an array
//!   `input` (else `400`).
//! * The `model` selects the answer ([`answer`]): the documented table
//!   below, else the assistant text `irp unknown model` (so a test whose
//!   model is misspelled never meets the failure it expects).
//! * [`OpenAiService::reset`] clears the per-case request count; the
//!   executor calls it before every case, so `irp-echo-requests` counts only
//!   the case's own earlier requests that reached the transport.
//! * Echo answers carry only bounded, non-secret projections of the request
//!   (never a held value): a key is answered as which variable it was.
//!
//! The `model` table, every answer a `200` Responses API result unless
//! stated:
//!
//! | `model` | answer |
//! |---|---|
//! | `irp-reply` | the assistant text `irp reply` |
//! | `irp-echo-prompt` | the `user` message, every line break written as `\|` |
//! | `irp-echo-system` | `none` without a `system` message; else the snapshot's active atomic states (`snapshot.root.configuration.atomic`), space-separated |
//! | `irp-echo-system-sections` | `none` without a `system` message; else `data_model`, `transitions`, `point` presence and value as `data_model=absent transitions=absent point=<options.point>` |
//! | `irp-echo-tools` | the tool names in order, comma-separated, or `none` |
//! | `irp-echo-tool-choice` | `tool_choice`, or `none` |
//! | `irp-echo-tool-parameters` | the first tool's parameter names, sorted, comma-separated, or `none` |
//! | `irp-echo-options` | `max_output_tokens=<n or none> reasoning=<effort or none>` |
//! | `irp-echo-authorization` | `key` for [`KEY`]'s value, `other-key` for [`OTHER_KEY`]'s |
//! | `irp-echo-url` | the request URL |
//! | `irp-echo-requests` | how many earlier requests of this case reached the transport |
//! | `irp-status-500` | status `500` |
//! | `irp-status-404` | status `404` |
//! | `irp-status-302` | status `302` (the transport answers carry no headers, so no `location`; the request count shows that nothing followed it) |
//! | `irp-transport-failure` | no response ([`ExtensionHttpFailure::Transport`]) |
//! | `irp-response-too-large` | no response ([`ExtensionHttpFailure::ResponseTooLarge`]) |
//! | `irp-not-json` | the body `not json` |
//! | `irp-no-output` | a result without `output` |
//! | `irp-no-message` | a result whose `output` is empty |
//! | `irp-user-message` | a `message` whose role is `user` |
//! | `irp-no-output-text` | an assistant `message` without `output_text` |
//! | `irp-call-first` | a call of the first tool with `{}` |
//! | `irp-call-first-data` | a call of the first tool with `{"data":"irp"}` |
//! | `irp-call-first-arguments` | a call of the first tool with `{"Var1":"irp"}` |
//! | `irp-call-first-delay` | a call of the first tool with `{"delay":"50ms"}` |
//! | `irp-call-first-internal` | a call of the first tool with `{"target":"#_internal"}` |
//! | `irp-call-first-with-text` | a call of the first tool with `{}` and the assistant text `irp reply` |
//! | `irp-call-first-if-required` | a call of the first tool with `{}` when `tool_choice` is `required`, else the assistant text `irp reply` |
//! | `irp-call-all` | one call of every tool, in order, each with `{}` |
//! | `irp-call-undeclared` | a call of `send_irp_undeclared` |
//! | `irp-call-first-then-undeclared` | a call of the first tool, then one of `send_irp_undeclared` |
//! | `irp-call-invalid-arguments` | a call of the first tool with the arguments `[]` |
//! | `irp-call-invalid-target` | a call of the first tool with `{"target":7}` |
//! | `irp-call-invalid-delay` | a call of the first tool with `{"delay":"soon"}` |
//! | `irp-no-call` | the assistant text `irp reply` and no call |

use std::sync::atomic::{AtomicU64, Ordering};

use scxml_host_wasmtime::{
    ExtensionHttp, ExtensionHttpFailure, ExtensionHttpMethod, ExtensionHttpRequest,
    ExtensionHttpResponse,
};

/// The variable whose fake value is the run's API key.
pub const KEY: &str = "IRP_OPENAI_KEY";
/// A second variable holding another fake key (slot binding is observable).
pub const OTHER_KEY: &str = "IRP_OTHER_KEY";
/// The variable whose value is the served base URL.
pub const BASE_URL_VARIABLE: &str = "IRP_BASE_URL";
/// The variable whose value is a base URL whose service is unreachable.
pub const UNREACHABLE_BASE_URL_VARIABLE: &str = "IRP_UNREACHABLE_BASE_URL";
/// The variable whose value supplies no origin.
pub const NO_ORIGIN_BASE_URL_VARIABLE: &str = "IRP_NO_ORIGIN_BASE_URL";

/// The fake value of [`KEY`].
const KEY_VALUE: &str = "sk-irp-test-key-0001";
/// The fake value of [`OTHER_KEY`].
const OTHER_KEY_VALUE: &str = "sk-irp-test-key-0002";
/// The served base URL (the value of [`BASE_URL_VARIABLE`]).
pub const BASE_URL: &str = "https://openai.irp.invalid/v1";
/// The unreachable base URL (the value of
/// [`UNREACHABLE_BASE_URL_VARIABLE`]).
pub const UNREACHABLE_BASE_URL: &str = "https://unreachable.irp.invalid/v1";
/// A base URL that supplies no origin (the value of
/// [`NO_ORIGIN_BASE_URL_VARIABLE`]).
const NO_ORIGIN_BASE_URL: &str = "irp/v1";
/// The package's declared endpoint (`https://api.openai.com`).
pub const DECLARED_URL: &str = "https://api.openai.com/v1/responses";

/// The values the run holds, by variable name: exactly these, never the
/// process environment.
#[must_use]
pub fn values() -> Vec<(String, String)> {
    [
        (KEY, KEY_VALUE),
        (OTHER_KEY, OTHER_KEY_VALUE),
        (BASE_URL_VARIABLE, BASE_URL),
        (UNREACHABLE_BASE_URL_VARIABLE, UNREACHABLE_BASE_URL),
        (NO_ORIGIN_BASE_URL_VARIABLE, NO_ORIGIN_BASE_URL),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect()
}

/// The stand-in transport (see the module documentation).
#[derive(Debug, Default)]
pub struct OpenAiService {
    /// Requests of the current case that reached the transport.
    requests: AtomicU64,
}

impl OpenAiService {
    /// A stand-in that has seen no request.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a new case: forgets the previous case's requests.
    pub fn reset(&self) {
        self.requests.store(0, Ordering::SeqCst);
    }
}

impl ExtensionHttp for OpenAiService {
    fn fetch(
        &self,
        request: &ExtensionHttpRequest,
    ) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
        let earlier = self.requests.fetch_add(1, Ordering::SeqCst);
        answer(request, earlier)
    }
}

/// One JSON response.
fn json(status: u16, body: &serde_json::Value) -> ExtensionHttpResponse {
    ExtensionHttpResponse {
        status,
        body: body.to_string().into_bytes(),
    }
}

/// A `200` Responses API result with `output`.
fn result(output: &serde_json::Value) -> ExtensionHttpResponse {
    json(
        200,
        &serde_json::json!({
            "id": "resp_irp",
            "object": "response",
            "status": "completed",
            "output": output,
        }),
    )
}

/// An assistant message carrying `text`.
fn message(text: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "message",
        "id": "msg_irp",
        "role": "assistant",
        "status": "completed",
        "content": [{ "type": "output_text", "text": text, "annotations": [] }],
    })
}

/// A function call of `name` with the raw `arguments` text.
fn call(name: &str, arguments: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "function_call",
        "id": "fc_irp",
        "call_id": "call_irp",
        "name": name,
        "arguments": arguments,
        "status": "completed",
    })
}

/// A `200` result with the assistant text `text`.
fn text(text: &str) -> ExtensionHttpResponse {
    result(&serde_json::json!([message(text)]))
}

/// An error status with a JSON error body.
fn error(status: u16, detail: &str) -> ExtensionHttpResponse {
    json(
        status,
        &serde_json::json!({ "error": { "message": detail } }),
    )
}

/// The answer to `request`, the case's `earlier`-th request that reached
/// the transport.
///
/// # Errors
///
/// [`ExtensionHttpFailure`] for the unreachable base URL and the models
/// that answer no response.
#[allow(
    clippy::too_many_lines,
    reason = "one match maps every documented model of the table to its answer"
)]
pub fn answer(
    request: &ExtensionHttpRequest,
    earlier: u64,
) -> Result<ExtensionHttpResponse, ExtensionHttpFailure> {
    if request.url == format!("{UNREACHABLE_BASE_URL}/responses") {
        return Err(ExtensionHttpFailure::Transport);
    }
    if request.url != DECLARED_URL && request.url != format!("{BASE_URL}/responses") {
        return Ok(error(404, "not found"));
    }
    if request.method != ExtensionHttpMethod::Post {
        return Ok(error(405, "method not allowed"));
    }
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|(field, _)| field.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    };
    let key = match header("authorization").and_then(|value| value.strip_prefix("Bearer ")) {
        Some(KEY_VALUE) => "key",
        Some(OTHER_KEY_VALUE) => "other-key",
        _ => return Ok(error(401, "unauthorized")),
    };
    if header("content-type") != Some("application/json") {
        return Ok(error(400, "the body must be JSON"));
    }
    let Ok(serde_json::Value::Object(body)) =
        serde_json::from_slice::<serde_json::Value>(&request.body)
    else {
        return Ok(error(400, "the body must be a JSON object"));
    };
    let (Some(model), Some(input)) = (
        body.get("model").and_then(serde_json::Value::as_str),
        body.get("input").and_then(serde_json::Value::as_array),
    ) else {
        return Ok(error(400, "the body needs a model and an input"));
    };
    let role = |wanted: &str| {
        input
            .iter()
            .find(|item| item.get("role").and_then(serde_json::Value::as_str) == Some(wanted))
            .and_then(|item| item.get("content"))
            .and_then(serde_json::Value::as_str)
    };
    let tools: Vec<&serde_json::Value> = body
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .map(|tools| tools.iter().collect())
        .unwrap_or_default();
    let tool_names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
        .collect();
    let first = tool_names.first().copied().unwrap_or("send_irp_none");
    let calls = |arguments: &str| result(&serde_json::json!([call(first, arguments)]));
    let or_none = |value: String| {
        if value.is_empty() {
            "none".into()
        } else {
            value
        }
    };
    Ok(match model {
        "irp-reply" | "irp-no-call" => text("irp reply"),
        "irp-echo-prompt" => text(&role("user").unwrap_or("").replace('\n', "|")),
        "irp-echo-system" => text(&match role("system")
            .map(serde_json::from_str::<serde_json::Value>)
        {
            None => "none".into(),
            Some(Ok(snapshot)) => snapshot
                .pointer("/snapshot/root/configuration/atomic")
                .and_then(serde_json::Value::as_array)
                .map_or_else(
                    || "no-configuration".into(),
                    |states| {
                        states
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .collect::<Vec<_>>()
                            .join(" ")
                    },
                ),
            Some(Err(_)) => "not-json".into(),
        }),
        "irp-echo-system-sections" => text(&match role("system")
            .map(serde_json::from_str::<serde_json::Value>)
        {
            None => "none".into(),
            Some(Ok(snapshot)) => {
                let present = |section: &str| {
                    if snapshot
                        .pointer(&format!("/snapshot/root/{section}"))
                        .is_some()
                    {
                        "present"
                    } else {
                        "absent"
                    }
                };
                format!(
                    "data_model={} transitions={} point={}",
                    present("data_model"),
                    present("transitions"),
                    snapshot
                        .pointer("/options/point")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("none")
                )
            }
            Some(Err(_)) => "not-json".into(),
        }),
        "irp-echo-tools" => text(&or_none(tool_names.join(","))),
        "irp-echo-tool-choice" => text(
            body.get("tool_choice")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none"),
        ),
        "irp-echo-tool-parameters" => text(&or_none(
            tools
                .first()
                .and_then(|tool| tool.pointer("/parameters/properties"))
                .and_then(serde_json::Value::as_object)
                .map(|properties| {
                    let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
                    names.sort_unstable();
                    names.join(",")
                })
                .unwrap_or_default(),
        )),
        "irp-echo-options" => text(&format!(
            "max_output_tokens={} reasoning={}",
            body.get("max_output_tokens")
                .and_then(serde_json::Value::as_u64)
                .map_or_else(|| "none".to_owned(), |tokens| tokens.to_string()),
            body.get("reasoning")
                .and_then(|reasoning| reasoning.get("effort"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("none")
        )),
        "irp-echo-authorization" => text(key),
        "irp-echo-url" => text(&request.url),
        "irp-echo-requests" => text(&earlier.to_string()),
        "irp-status-500" => error(500, "internal error"),
        "irp-status-404" => error(404, "not found"),
        "irp-status-302" => ExtensionHttpResponse {
            status: 302,
            body: b"{}".to_vec(),
        },
        "irp-transport-failure" => return Err(ExtensionHttpFailure::Transport),
        "irp-response-too-large" => return Err(ExtensionHttpFailure::ResponseTooLarge),
        "irp-not-json" => ExtensionHttpResponse {
            status: 200,
            body: b"not json".to_vec(),
        },
        "irp-no-output" => json(200, &serde_json::json!({ "object": "response" })),
        "irp-no-message" => result(&serde_json::json!([])),
        "irp-user-message" => result(&serde_json::json!([{
            "type": "message",
            "id": "msg_irp",
            "role": "user",
            "content": [{ "type": "output_text", "text": "irp reply" }],
        }])),
        "irp-no-output-text" => result(&serde_json::json!([{
            "type": "message",
            "id": "msg_irp",
            "role": "assistant",
            "content": [],
        }])),
        "irp-call-first" => calls("{}"),
        "irp-call-first-data" => calls(r#"{"data":"irp"}"#),
        "irp-call-first-arguments" => calls(r#"{"Var1":"irp"}"#),
        "irp-call-first-delay" => calls(r#"{"delay":"50ms"}"#),
        "irp-call-first-internal" => calls(r##"{"target":"#_internal"}"##),
        "irp-call-first-if-required" => {
            if body.get("tool_choice").and_then(serde_json::Value::as_str) == Some("required") {
                calls("{}")
            } else {
                text("irp reply")
            }
        }
        "irp-call-first-with-text" => result(&serde_json::json!([
            call(first, "{}"),
            message("irp reply")
        ])),
        "irp-call-all" => result(
            &tool_names
                .iter()
                .map(|name| call(name, "{}"))
                .collect::<serde_json::Value>(),
        ),
        "irp-call-undeclared" => result(&serde_json::json!([call("send_irp_undeclared", "{}")])),
        "irp-call-first-then-undeclared" => result(&serde_json::json!([
            call(first, "{}"),
            call("send_irp_undeclared", "{}")
        ])),
        "irp-call-invalid-arguments" => calls("[]"),
        "irp-call-invalid-target" => calls(r#"{"target":7}"#),
        "irp-call-invalid-delay" => calls(r#"{"delay":"soon"}"#),
        _ => text("irp unknown model"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str, authorization: &str, body: &serde_json::Value) -> ExtensionHttpRequest {
        ExtensionHttpRequest {
            method: ExtensionHttpMethod::Post,
            url: url.into(),
            headers: vec![
                ("authorization".into(), authorization.into()),
                ("content-type".into(), "application/json".into()),
            ],
            body: body.to_string().into_bytes(),
        }
    }

    fn reply(response: &ExtensionHttpResponse) -> String {
        let body: serde_json::Value =
            serde_json::from_slice(&response.body).expect("a JSON answer");
        body.pointer("/output/0/content/0/text")
            .and_then(serde_json::Value::as_str)
            .expect("an assistant text")
            .to_owned()
    }

    fn model(model: &str) -> serde_json::Value {
        serde_json::json!({ "model": model, "input": [{ "role": "user", "content": "a\nb" }] })
    }

    #[test]
    fn served_urls_require_a_held_key_and_answer_by_model() {
        let bearer = format!("Bearer {KEY_VALUE}");
        for url in [DECLARED_URL.to_owned(), format!("{BASE_URL}/responses")] {
            let answered = answer(&request(&url, &bearer, &model("irp-reply")), 0)
                .expect("a served URL answers");
            assert_eq!(
                (answered.status, reply(&answered)),
                (200, "irp reply".into())
            );
        }
        let echoed = answer(
            &request(DECLARED_URL, &bearer, &model("irp-echo-prompt")),
            0,
        )
        .expect("an echo answers");
        assert_eq!(reply(&echoed), "a|b");
        let other = answer(
            &request(
                DECLARED_URL,
                &format!("Bearer {OTHER_KEY_VALUE}"),
                &model("irp-echo-authorization"),
            ),
            0,
        )
        .expect("the other key answers");
        assert_eq!(reply(&other), "other-key");
        for (url, authorization, expected) in [
            (DECLARED_URL, "Bearer sk-wrong", 401),
            ("https://api.openai.com/v1/other", bearer.as_str(), 404),
        ] {
            let refused = answer(&request(url, authorization, &model("irp-reply")), 0)
                .expect("an HTTP answer");
            assert_eq!(refused.status, expected, "{url} {authorization}");
        }
        assert_eq!(
            reply(
                &answer(&request(DECLARED_URL, &bearer, &model("irp-unknown")), 0)
                    .expect("an HTTP answer")
            ),
            "irp unknown model",
            "a misspelled model never meets an expected failure"
        );
        assert_eq!(
            answer(
                &request(
                    &format!("{UNREACHABLE_BASE_URL}/responses"),
                    &bearer,
                    &model("irp-reply")
                ),
                0
            ),
            Err(ExtensionHttpFailure::Transport)
        );
    }

    #[test]
    fn the_request_count_is_per_case() {
        let service = OpenAiService::new();
        let bearer = format!("Bearer {KEY_VALUE}");
        let echo = request(DECLARED_URL, &bearer, &model("irp-echo-requests"));
        assert_eq!(reply(&service.fetch(&echo).expect("answers")), "0");
        assert_eq!(reply(&service.fetch(&echo).expect("answers")), "1");
        service.reset();
        assert_eq!(reply(&service.fetch(&echo).expect("answers")), "0");
    }

    #[test]
    fn the_held_values_are_fake_and_distinct() {
        let values = values();
        assert_eq!(values.len(), 5);
        assert!(values.iter().all(|(name, _)| name.starts_with("IRP_")));
        assert!(
            values
                .iter()
                .all(|(_, value)| !value.contains("sk-") || value.starts_with("sk-irp-test-"))
        );
    }
}
