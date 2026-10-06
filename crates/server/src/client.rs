//! What the server sends the client on its own: notifications and requests with ids of its own.

use crossbeam_channel::Sender;
use lsp_server::{Message, Notification, Request, RequestId};
use lsp_types::notification::{LogMessage, Notification as _};
use lsp_types::{LogMessageParams, MessageType};

use crate::BoxError;

/// The way to the client, with the counter of the ids of the requests the server asks it.
pub struct Client {
    sender: Sender<Message>,
    last_id: i32,
}

impl Client {
    pub fn new(sender: Sender<Message>) -> Client {
        Client { sender, last_id: 0 }
    }

    pub fn send(&self, message: impl Into<Message>) -> Result<(), BoxError> {
        self.sender.send(message.into())?;
        Ok(())
    }

    /// An id no request of this server used before, counting up from 1.
    pub fn next_id(&mut self) -> RequestId {
        self.last_id += 1;
        RequestId::from(self.last_id)
    }

    /// Asks the client something and gives the id its answer comes back with.
    pub fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> Result<RequestId, BoxError> {
        let id = self.next_id();
        self.send(Request::new(id.clone(), R::METHOD.to_string(), params))?;
        Ok(id)
    }

    pub fn notify<N: lsp_types::notification::Notification>(&self, params: N::Params) -> Result<(), BoxError> {
        self.send(Notification::new(N::METHOD.to_string(), params))
    }

    /// Writes to the client's log. A client that is gone has no log, so a failure is no error.
    pub fn log(&self, kind: MessageType, message: String) {
        let _ = self.send(Notification::new(
            LogMessage::METHOD.to_string(),
            LogMessageParams { typ: kind, message },
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::notification::PublishDiagnostics;
    use lsp_types::request::{SemanticTokensRefresh, WorkspaceConfiguration};
    use lsp_types::{ConfigurationItem, ConfigurationParams, PublishDiagnosticsParams, Uri};
    use serde_json::json;
    use std::str::FromStr;

    #[test]
    fn numbers_requests_from_one() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut client = Client::new(sender);
        let first = client
            .request::<WorkspaceConfiguration>(ConfigurationParams {
                items: vec![ConfigurationItem {
                    scope_uri: None,
                    section: Some("sql".to_string()),
                }],
            })
            .expect("sent");
        let second = client.request::<SemanticTokensRefresh>(()).expect("sent");
        assert_eq!(client.next_id(), RequestId::from(3));
        assert_eq!((first, second), (RequestId::from(1), RequestId::from(2)));
        let Ok(Message::Request(request)) = receiver.try_recv() else {
            panic!("a request");
        };
        assert_eq!(request.method, "workspace/configuration");
        assert_eq!(request.params, json!({ "items": [{ "section": "sql" }] }));
        let Ok(Message::Request(request)) = receiver.try_recv() else {
            panic!("a request");
        };
        assert_eq!(
            (request.method.as_str(), request.params),
            ("workspace/semanticTokens/refresh", json!(null))
        );
    }

    #[test]
    fn sends_notifications_and_log_lines() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        let client = Client::new(sender);
        let uri = Uri::from_str("file:///a.sql").expect("a uri");
        client
            .notify::<PublishDiagnostics>(PublishDiagnosticsParams::new(uri, Vec::new(), Some(4)))
            .expect("sent");
        client.log(MessageType::INFO, "hello".to_string());
        let messages: Vec<Message> = receiver.try_iter().collect();
        let [Message::Notification(diagnostics), Message::Notification(log)] = &messages[..] else {
            panic!("two notifications: {messages:?}");
        };
        assert_eq!(diagnostics.method, "textDocument/publishDiagnostics");
        assert_eq!(diagnostics.params["version"], 4);
        assert_eq!(log.method, "window/logMessage");
        assert_eq!(log.params, json!({ "type": 3, "message": "hello" }));
    }

    #[test]
    fn a_client_that_is_gone_is_an_error_except_for_the_log() {
        let (sender, receiver) = crossbeam_channel::unbounded();
        drop(receiver);
        let mut client = Client::new(sender);
        assert!(client.request::<SemanticTokensRefresh>(()).is_err());
        client.log(MessageType::INFO, "nobody reads this".to_string());
    }
}
