//! Talks to a server over an in-memory connection, the way an editor talks to it over stdio.

use std::thread::JoinHandle;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use serde_json::{Value, json};

use crate::BoxError;

/// How long a test waits for the server before it fails.
pub const TIMEOUT: Duration = Duration::from_secs(10);

pub struct TestClient {
    pub connection: Connection,
    pub server: Option<JoinHandle<()>>,
    pub next_id: i32,
    /// Notifications and requests seen while waiting for something else.
    pub backlog: Vec<Message>,
}

impl TestClient {
    /// Starts `run` on a thread, sends `initialize` with these parameters and `initialized`, and
    /// gives the result of `initialize`.
    pub fn connect(
        run: impl FnOnce(Connection) -> Result<(), BoxError> + Send + 'static,
        initialize: Value,
    ) -> (TestClient, Value) {
        let (server_side, client_side) = Connection::memory();
        let server = std::thread::spawn(move || {
            run(server_side).expect("the server runs to the end");
        });
        let mut client = TestClient {
            connection: client_side,
            server: Some(server),
            next_id: 0,
            backlog: Vec::new(),
        };
        let result = client.request("initialize", initialize);
        client.notify("initialized", json!({}));
        (client, result)
    }

    pub fn send(&self, message: impl Into<Message>) {
        self.connection
            .sender
            .send(message.into())
            .expect("the server is listening");
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.send(Notification::new(method.to_string(), params));
    }

    /// The result a request answers with.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        self.response(method, params)
            .response_result
            .expect("the request succeeds")
    }

    /// The message of the error a request answers with.
    pub fn request_error(&mut self, method: &str, params: Value) -> String {
        self.response(method, params)
            .response_result
            .expect_err("the request fails")
            .message
    }

    fn response(&mut self, method: &str, params: Value) -> Response {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        self.send(Request::new(id.clone(), method.to_string(), params));
        loop {
            match self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("the server answers")
            {
                Message::Response(response) if response.id == id => return response,
                other => self.backlog.push(other),
            }
        }
    }

    /// The next message that `pick` accepts, from the backlog first.
    pub fn wait_for<T>(&mut self, mut pick: impl FnMut(&Message) -> Option<T>) -> T {
        if let Some(position) = self.backlog.iter().position(|message| pick(message).is_some()) {
            let message = self.backlog.remove(position);
            return pick(&message).expect("picked above");
        }
        loop {
            let message = self
                .connection
                .receiver
                .recv_timeout(TIMEOUT)
                .expect("a message arrives");
            if let Some(found) = pick(&message) {
                return found;
            }
            self.backlog.push(message);
        }
    }

    pub fn open_document(&self, uri: &str, language_id: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            json!({ "textDocument": { "uri": uri, "languageId": language_id, "version": 1, "text": text } }),
        );
    }

    /// Asks `method` at a position of a document.
    pub fn at(&mut self, method: &str, uri: &str, line: u32, character: u32) -> Value {
        self.request(
            method,
            json!({ "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }),
        )
    }

    /// The next diagnostics published for a document.
    pub fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        self.wait_for(|message| match message {
            Message::Notification(notification) if notification.method == "textDocument/publishDiagnostics" => {
                (notification.params["uri"] == uri).then(|| {
                    notification.params["diagnostics"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                })
            }
            _ => None,
        })
    }

    /// Agrees to every request to create a progress and waits until a progress ends. Gives the
    /// kinds of the `$/progress` notifications on the way, `end` last.
    pub fn wait_for_progress_end(&mut self) -> Vec<String> {
        let mut kinds = Vec::new();
        loop {
            let message = self.wait_for(|message| match message {
                Message::Request(request) if request.method == "window/workDoneProgress/create" => {
                    Some(message.clone())
                }
                Message::Notification(notification) if notification.method == "$/progress" => Some(message.clone()),
                _ => None,
            });
            match message {
                Message::Request(request) => self.send(Response::new_ok(request.id, Value::Null)),
                Message::Notification(notification) => {
                    let kind = notification.params["value"]["kind"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let done = kind == "end";
                    kinds.push(kind);
                    if done {
                        return kinds;
                    }
                }
                Message::Response(_) => {}
            }
        }
    }

    /// Shuts the server down and waits for its thread to end.
    pub fn shutdown(mut self) {
        self.request("shutdown", Value::Null);
        self.notify("exit", Value::Null);
        self.server
            .take()
            .expect("started")
            .join()
            .expect("the server stops cleanly");
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::{
        Client, Documents, Handler, PositionEncoding, Progress, ProgressLabels, answer, main_loop, unsupported,
    };
    use crossbeam_channel::Sender;
    use lsp_types::notification::{DidChangeTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics};
    use lsp_types::request::{HoverRequest, Request as _};
    use lsp_types::{
        Diagnostic, DidChangeTextDocumentParams, DidOpenTextDocumentParams, Hover, HoverContents, HoverParams,
        InitializeResult, MarkedString, PublishDiagnosticsParams, Uri,
    };

    /// A server of words: it counts the words of a document, answers hover with the word under
    /// the cursor and reports every word `bad`, which is enough to drive everything the crate offers.
    struct Words {
        client: Client,
        documents: Documents<Vec<(u32, u32)>>,
        encoding: PositionEncoding,
        progress: Progress,
        dirty: Vec<Uri>,
        jobs: Sender<u32>,
    }

    fn words(text: &str) -> Vec<(u32, u32)> {
        let mut found = Vec::new();
        let mut start = None;
        for (offset, character) in text.char_indices().chain([(text.len(), ' ')]) {
            match (start, character.is_alphanumeric()) {
                (None, true) => start = Some(offset),
                (Some(from), false) => {
                    found.push((from as u32, offset as u32));
                    start = None;
                }
                _ => {}
            }
        }
        found
    }

    impl Words {
        fn hover(&mut self, params: HoverParams) -> Option<Hover> {
            let position = params.text_document_position_params;
            let encoding = self.encoding;
            let document = self.documents.get_mut(&position.text_document.uri)?;
            let offset = u32::from(document.mapper(encoding).offset(position.position));
            let (start, end) = *document
                .parse_with(words)
                .iter()
                .find(|(start, end)| (*start..=*end).contains(&offset))?;
            Some(Hover {
                contents: HoverContents::Scalar(MarkedString::String(
                    document.text[start as usize..end as usize].to_string(),
                )),
                range: None,
            })
        }
    }

    impl Handler for Words {
        type Event = u32;

        fn request(&mut self, request: Request) -> Result<(), BoxError> {
            let response = match request.method.as_str() {
                HoverRequest::METHOD => answer(request.id, request.params, |params| self.hover(params)),
                method => unsupported(request.id, method),
            };
            self.client.send(response)
        }

        fn notification(&mut self, notification: Notification) -> Result<(), BoxError> {
            match notification.method.as_str() {
                DidOpenTextDocument::METHOD => {
                    let params: DidOpenTextDocumentParams = serde_json::from_value(notification.params)?;
                    let item = params.text_document;
                    self.documents.open(item.uri.clone(), item.version, item.text);
                    self.dirty.push(item.uri);
                    self.progress.job_started(&mut self.client);
                    self.jobs.send(1)?;
                }
                DidChangeTextDocument::METHOD => {
                    let params: DidChangeTextDocumentParams = serde_json::from_value(notification.params)?;
                    let uri = params.text_document.uri;
                    if let Some(document) = self.documents.get_mut(&uri) {
                        document.apply_changes(params.text_document.version, &params.content_changes, self.encoding);
                        self.dirty.push(uri);
                    }
                }
                _ => {}
            }
            Ok(())
        }

        fn response(&mut self, response: Response) -> Result<(), BoxError> {
            self.progress.response(&self.client, &response);
            Ok(())
        }

        fn event(&mut self, files: u32) -> Result<(), BoxError> {
            self.progress.job_progress(&self.client, files as usize, files as usize);
            self.progress.job_finished(&self.client);
            Ok(())
        }

        fn idle(&mut self) -> Result<(), BoxError> {
            for uri in std::mem::take(&mut self.dirty) {
                let Some(document) = self.documents.get_mut(&uri) else {
                    continue;
                };
                let version = document.version;
                let found: Vec<(u32, u32)> = document.parse_with(words).clone();
                let mapper = document.mapper(self.encoding);
                let diagnostics = found
                    .into_iter()
                    .filter(|(start, end)| &document.text[*start as usize..*end as usize] == "bad")
                    .map(|(start, end)| Diagnostic {
                        range: mapper.range(crate::TextRange::new(start.into(), end.into())),
                        message: "A bad word".to_string(),
                        ..Diagnostic::default()
                    })
                    .collect();
                self.client.notify::<PublishDiagnostics>(PublishDiagnosticsParams::new(
                    uri,
                    diagnostics,
                    Some(version),
                ))?;
            }
            Ok(())
        }
    }

    fn run(connection: Connection) -> Result<(), BoxError> {
        let (id, params) = connection.initialize_start()?;
        let params: lsp_types::InitializeParams = serde_json::from_value(params)?;
        let encoding = crate::choose_encoding(
            params
                .capabilities
                .general
                .as_ref()
                .and_then(|general| general.position_encodings.as_deref()),
        );
        let result = InitializeResult {
            capabilities: lsp_types::ServerCapabilities {
                position_encoding: Some(crate::encoding_kind(encoding)),
                ..Default::default()
            },
            server_info: None,
        };
        connection.initialize_finish(id, serde_json::to_value(result)?)?;
        let (jobs, events) = crossbeam_channel::unbounded();
        let supported = params
            .capabilities
            .window
            .and_then(|window| window.work_done_progress)
            .unwrap_or(false);
        let mut server = Words {
            client: Client::new(connection.sender.clone()),
            documents: Documents::default(),
            encoding,
            progress: Progress::new(
                supported,
                ProgressLabels {
                    token: "words/reading".to_string(),
                    title: "Reading words".to_string(),
                    unit: "documents".to_string(),
                    done: "Read".to_string(),
                },
            ),
            dirty: Vec::new(),
            jobs,
        };
        main_loop(&connection, &events, &mut server)
    }

    fn start(capabilities: Value) -> (TestClient, Value) {
        TestClient::connect(run, json!({ "processId": null, "capabilities": capabilities }))
    }

    const URI: &str = "file:///words.txt";

    #[test]
    fn drives_a_server_through_documents_requests_progress_and_diagnostics() {
        let (mut client, result) = start(json!({
            "general": { "positionEncodings": ["utf-16", "utf-8"] },
            "window": { "workDoneProgress": true }
        }));
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-8");
        client.open_document(URI, "plaintext", "good \u{e9}t\u{e9} bad");
        let kinds = client.wait_for_progress_end();
        assert_eq!(kinds.first().map(String::as_str), Some("begin"), "{kinds:?}");
        let diagnostics = client.diagnostics(URI);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["range"]["start"], json!({ "line": 0, "character": 11 }));
        assert_eq!(client.at("textDocument/hover", URI, 0, 7)["contents"], "\u{e9}t\u{e9}");
        client.notify(
            "textDocument/didChange",
            json!({
                "textDocument": { "uri": URI, "version": 2 },
                "contentChanges": [{ "range": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 14 } }, "text": "fine" }]
            }),
        );
        assert!(client.diagnostics(URI).is_empty());
        assert_eq!(client.at("textDocument/hover", URI, 0, 13)["contents"], "fine");
        assert!(
            client
                .request_error("words/count", json!({}))
                .contains("unsupported request")
        );
        assert!(
            client
                .request_error("textDocument/hover", json!({ "nonsense": 1 }))
                .contains("missing field")
        );
        client.shutdown();
    }

    #[test]
    fn negotiates_utf16_for_a_client_that_offers_nothing() {
        let (mut client, result) = start(json!({}));
        assert_eq!(result["capabilities"]["positionEncoding"], "utf-16");
        client.open_document(URI, "plaintext", "\u{1f600} bad");
        let diagnostics = client.diagnostics(URI);
        assert_eq!(diagnostics[0]["range"]["start"]["character"], 3);
        assert_eq!(client.at("textDocument/hover", URI, 0, 4)["contents"], "bad");
        assert_eq!(client.at("textDocument/hover", URI, 0, 2), Value::Null);
        let uri = Uri::from_str(URI).expect("a uri");
        assert_eq!(
            crate::paths::uri_to_path(&uri).expect("a path").to_str(),
            Some("/words.txt")
        );
        client.shutdown();
    }
}
