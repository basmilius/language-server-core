//! Background work of the server as one work done progress a client can show, however many jobs
//! run at once.

use lsp_server::{RequestId, Response};
use lsp_types::notification::Progress as ProgressNotification;
use lsp_types::request::WorkDoneProgressCreate;
use lsp_types::{
    NumberOrString, ProgressParams, ProgressParamsValue, WorkDoneProgress, WorkDoneProgressBegin,
    WorkDoneProgressCreateParams, WorkDoneProgressEnd, WorkDoneProgressReport,
};

use crate::client::Client;

/// The words of a progress.
#[derive(Clone, Debug)]
pub struct ProgressLabels {
    /// The prefix of every token, followed by `/` and a counter.
    pub token: String,
    /// The title the client shows, such as `Indexing files`.
    pub title: String,
    /// What the work counts, in a report of `3 of 10 files`.
    pub unit: String,
    /// The message the progress ends with.
    pub done: String,
}

/// The jobs in flight and how far they are. The client is asked to create a token when the first
/// job starts; the progress begins once it says yes and ends when the last job is finished.
pub struct Progress {
    supported: bool,
    labels: ProgressLabels,
    counter: u32,
    token: Option<String>,
    /// The client answered `window/workDoneProgress/create` and the report has begun.
    begun: bool,
    create_request: Option<RequestId>,
    running_jobs: usize,
    total: usize,
    done: usize,
    last_percent: u32,
}

impl Progress {
    /// A progress for a client that announced `window.workDoneProgress`, or one that tells it
    /// nothing when `supported` is false.
    pub fn new(supported: bool, labels: ProgressLabels) -> Progress {
        Progress {
            supported,
            labels,
            counter: 0,
            token: None,
            begun: false,
            create_request: None,
            running_jobs: 0,
            total: 0,
            done: 0,
            last_percent: 0,
        }
    }

    pub fn running_jobs(&self) -> usize {
        self.running_jobs
    }

    pub fn job_started(&mut self, client: &mut Client) {
        self.running_jobs += 1;
        if self.supported && self.token.is_none() {
            self.counter += 1;
            let token = format!("{}/{}", self.labels.token, self.counter);
            let id = client.next_id();
            self.token = Some(token.clone());
            self.begun = false;
            self.create_request = Some(id.clone());
            let _ = client.send(lsp_server::Request::new(
                id,
                <WorkDoneProgressCreate as lsp_types::request::Request>::METHOD.to_string(),
                WorkDoneProgressCreateParams {
                    token: NumberOrString::String(token),
                },
            ));
        }
    }

    /// Counts `done` units of work done and `discovered` more to do. A report goes out when the
    /// percentage changes.
    pub fn job_progress(&mut self, client: &Client, done: usize, discovered: usize) {
        self.total += discovered;
        self.done += done;
        let percent = self.percent();
        if self.begun && percent != self.last_percent {
            self.last_percent = percent;
            self.send(
                client,
                WorkDoneProgress::Report(WorkDoneProgressReport {
                    cancellable: Some(false),
                    message: Some(format!("{} of {} {}", self.done, self.total, self.labels.unit)),
                    percentage: Some(percent),
                }),
            );
        }
    }

    pub fn job_finished(&mut self, client: &Client) {
        self.running_jobs = self.running_jobs.saturating_sub(1);
        // While the client has not answered the request to create the progress, its answer ends it.
        if self.running_jobs > 0 || self.create_request.is_some() {
            return;
        }
        if self.begun {
            self.send(
                client,
                WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(self.labels.done.clone()),
                }),
            );
        }
        self.token = None;
        self.begun = false;
        self.total = 0;
        self.done = 0;
        self.last_percent = 0;
    }

    /// Takes the client's answer to the request to create the progress, and says whether the
    /// response was that answer.
    pub fn response(&mut self, client: &Client, response: &Response) -> bool {
        if self.create_request.as_ref() != Some(&response.id) {
            return false;
        }
        self.create_request = None;
        if response.response_result.is_err() {
            self.token = None;
            return true;
        }
        self.begun = true;
        self.last_percent = self.percent();
        self.send(
            client,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: self.labels.title.clone(),
                cancellable: Some(false),
                message: None,
                percentage: Some(self.last_percent),
            }),
        );
        // The work was done before the client answered: report it as done right away.
        if self.running_jobs == 0 {
            self.send(
                client,
                WorkDoneProgress::End(WorkDoneProgressEnd {
                    message: Some(self.labels.done.clone()),
                }),
            );
            self.token = None;
            self.begun = false;
        }
        true
    }

    fn percent(&self) -> u32 {
        if self.total == 0 {
            return 0;
        }
        ((self.done * 100) / self.total).min(100) as u32
    }

    fn send(&self, client: &Client, value: WorkDoneProgress) {
        let Some(token) = &self.token else {
            return;
        };
        let _ = client.notify::<ProgressNotification>(ProgressParams {
            token: NumberOrString::String(token.clone()),
            value: ProgressParamsValue::WorkDone(value),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Receiver;
    use lsp_server::{ErrorCode, Message};
    use serde_json::{Value, json};

    fn labels() -> ProgressLabels {
        ProgressLabels {
            token: "sql-language-server/indexing".to_string(),
            title: "Indexing SQL files".to_string(),
            unit: "files".to_string(),
            done: "Indexed".to_string(),
        }
    }

    fn setup(supported: bool) -> (Progress, Client, Receiver<Message>) {
        let (sender, receiver) = crossbeam_channel::unbounded();
        (Progress::new(supported, labels()), Client::new(sender), receiver)
    }

    /// What was sent, as `create <token>` or the kind of a `$/progress` with what it says.
    fn sent(receiver: &Receiver<Message>) -> Vec<String> {
        receiver
            .try_iter()
            .map(|message| match message {
                Message::Request(request) => {
                    assert_eq!(request.method, "window/workDoneProgress/create");
                    format!(
                        "create {} as {}",
                        request.params["token"].as_str().unwrap_or(""),
                        request.id
                    )
                }
                Message::Notification(notification) => {
                    assert_eq!(notification.method, "$/progress");
                    let value = &notification.params["value"];
                    let mut line = value["kind"].as_str().unwrap_or("").to_string();
                    for key in ["title", "message", "percentage"] {
                        if let Some(field) = value.get(key) {
                            line.push_str(&format!(" {key}={}", field_text(field)));
                        }
                    }
                    line
                }
                Message::Response(_) => panic!("the server sends no responses here"),
            })
            .collect()
    }

    fn field_text(value: &Value) -> String {
        value.as_str().map_or_else(|| value.to_string(), str::to_string)
    }

    fn ok(id: i32) -> Response {
        Response::new_ok(RequestId::from(id), json!(null))
    }

    #[test]
    fn begins_when_the_client_agrees_reports_and_ends_after_the_last_job() {
        let (mut progress, mut client, receiver) = setup(true);
        progress.job_started(&mut client);
        progress.job_started(&mut client);
        assert_eq!(sent(&receiver), ["create sql-language-server/indexing/1 as 1"]);
        progress.job_progress(&client, 0, 4);
        assert!(
            sent(&receiver).is_empty(),
            "nothing is reported before the progress began"
        );
        assert!(progress.response(&client, &ok(1)));
        progress.job_progress(&client, 1, 0);
        progress.job_progress(&client, 0, 0);
        progress.job_finished(&client);
        progress.job_progress(&client, 3, 0);
        progress.job_finished(&client);
        assert_eq!(
            sent(&receiver),
            [
                "begin title=Indexing SQL files percentage=0",
                "report message=1 of 4 files percentage=25",
                "report message=4 of 4 files percentage=100",
                "end message=Indexed",
            ]
        );
        assert_eq!(progress.running_jobs(), 0);
    }

    #[test]
    fn a_new_job_after_the_end_starts_a_new_token() {
        let (mut progress, mut client, receiver) = setup(true);
        progress.job_started(&mut client);
        progress.response(&client, &ok(1));
        progress.job_finished(&client);
        progress.job_started(&mut client);
        let sent = sent(&receiver);
        assert_eq!(
            sent.last().map(String::as_str),
            Some("create sql-language-server/indexing/2 as 2")
        );
    }

    #[test]
    fn work_finished_before_the_client_answered_ends_with_the_answer() {
        let (mut progress, mut client, receiver) = setup(true);
        progress.job_started(&mut client);
        progress.job_progress(&client, 2, 2);
        progress.job_finished(&client);
        assert_eq!(sent(&receiver), ["create sql-language-server/indexing/1 as 1"]);
        assert!(progress.response(&client, &ok(1)));
        assert_eq!(
            sent(&receiver),
            ["begin title=Indexing SQL files percentage=100", "end message=Indexed"]
        );
    }

    #[test]
    fn a_refused_token_reports_nothing() {
        let (mut progress, mut client, receiver) = setup(true);
        progress.job_started(&mut client);
        let refused = Response::new_err(RequestId::from(1), ErrorCode::InternalError as i32, "no".to_string());
        assert!(progress.response(&client, &refused));
        progress.job_progress(&client, 1, 1);
        progress.job_finished(&client);
        assert_eq!(sent(&receiver), ["create sql-language-server/indexing/1 as 1"]);
    }

    #[test]
    fn other_responses_are_not_its_own() {
        let (mut progress, mut client, _receiver) = setup(true);
        assert!(!progress.response(&client, &ok(1)));
        progress.job_started(&mut client);
        assert!(!progress.response(&client, &ok(9)));
        assert!(progress.response(&client, &ok(1)));
        assert!(!progress.response(&client, &ok(1)));
    }

    #[test]
    fn a_client_without_support_hears_nothing_and_the_jobs_still_count() {
        let (mut progress, mut client, receiver) = setup(false);
        progress.job_started(&mut client);
        progress.job_progress(&client, 1, 2);
        assert_eq!(progress.running_jobs(), 1);
        progress.job_finished(&client);
        progress.job_finished(&client);
        assert_eq!(progress.running_jobs(), 0);
        assert!(sent(&receiver).is_empty());
        assert_eq!(client.next_id(), RequestId::from(1));
    }

    #[test]
    fn the_percentage_never_passes_a_hundred() {
        let (mut progress, mut client, receiver) = setup(true);
        progress.job_started(&mut client);
        progress.response(&client, &ok(1));
        progress.job_progress(&client, 5, 2);
        assert_eq!(sent(&receiver)[2..], ["report message=5 of 2 files percentage=100"]);
    }
}
