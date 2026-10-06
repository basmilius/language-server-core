//! From a request to the handler that answers it, and from what the handler returns to a response.

use lsp_server::{ErrorCode, RequestId, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Decodes the parameters, runs a handler and wraps what it returns in a response. Parameters
/// that do not decode are answered with `InvalidParams`, without running the handler.
pub fn answer<P, R>(id: RequestId, params: Value, handler: impl FnOnce(P) -> Option<R>) -> Response
where
    P: DeserializeOwned,
    R: Serialize,
{
    match serde_json::from_value::<P>(params) {
        Ok(params) => Response::new_ok(id, handler(params)),
        Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
    }
}

/// Like [`answer`] for a handler that can refuse, with a message the client shows, as
/// `RequestFailed`.
pub fn answer_checked<P, R>(
    id: RequestId,
    params: Value,
    handler: impl FnOnce(P) -> Result<Option<R>, String>,
) -> Response
where
    P: DeserializeOwned,
    R: Serialize,
{
    match serde_json::from_value::<P>(params) {
        Ok(params) => match handler(params) {
            Ok(result) => Response::new_ok(id, result),
            Err(message) => Response::new_err(id, ErrorCode::RequestFailed as i32, message),
        },
        Err(error) => Response::new_err(id, ErrorCode::InvalidParams as i32, error.to_string()),
    }
}

/// The answer to a request the server does not know.
pub fn unsupported(id: RequestId, method: &str) -> Response {
    Response::new_err(
        id,
        ErrorCode::MethodNotFound as i32,
        format!("unsupported request {method}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position, TextDocumentPositionParams};
    use serde_json::json;

    fn params() -> Value {
        json!({ "textDocument": { "uri": "file:///a.sql" }, "position": { "line": 1, "character": 2 } })
    }

    fn error(response: Response) -> (i32, String) {
        let error = response.response_result.expect_err("an error");
        (error.code, error.message)
    }

    #[test]
    fn answers_with_what_the_handler_returns() {
        let response = answer(RequestId::from(7), params(), |params: TextDocumentPositionParams| {
            Some(params.position)
        });
        assert_eq!(response.id, RequestId::from(7));
        assert_eq!(
            response.response_result.expect("a result"),
            json!({ "line": 1, "character": 2 })
        );
    }

    #[test]
    fn nothing_to_answer_is_null() {
        let response = answer(RequestId::from(1), params(), |_: TextDocumentPositionParams| {
            None::<Position>
        });
        assert_eq!(response.response_result.expect("a result"), Value::Null);
    }

    #[test]
    fn parameters_that_do_not_decode_are_invalid_and_never_reach_the_handler() {
        let response = answer(
            RequestId::from(1),
            json!({ "nope": true }),
            |_: TextDocumentPositionParams| -> Option<()> { panic!("not run") },
        );
        let (code, message) = error(response);
        assert_eq!(code, ErrorCode::InvalidParams as i32);
        assert!(message.contains("textDocument"), "{message}");
        let response = answer_checked(
            RequestId::from(1),
            json!(3),
            |_: TextDocumentPositionParams| -> Result<Option<()>, String> { panic!("not run") },
        );
        assert_eq!(error(response).0, ErrorCode::InvalidParams as i32);
    }

    #[test]
    fn a_refusal_is_a_failed_request_with_its_message() {
        let response = answer_checked(RequestId::from(2), params(), |_: TextDocumentPositionParams| {
            Err::<Option<()>, _>("Cannot rename this".to_string())
        });
        assert_eq!(
            error(response),
            (ErrorCode::RequestFailed as i32, "Cannot rename this".to_string())
        );
        let response = answer_checked(RequestId::from(2), params(), |params: TextDocumentPositionParams| {
            Ok(Some(params.position.line))
        });
        assert_eq!(response.response_result.expect("a result"), json!(1));
    }

    #[test]
    fn an_unknown_request_is_not_found() {
        assert_eq!(
            error(unsupported(RequestId::from(3), "sql/explain")),
            (
                ErrorCode::MethodNotFound as i32,
                "unsupported request sql/explain".to_string()
            )
        );
    }
}
