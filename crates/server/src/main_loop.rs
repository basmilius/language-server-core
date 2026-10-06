//! The loop that hands every message of the client and every event of the server's own background
//! work to the server, until the client shuts it down.

use crossbeam_channel::{Receiver, select};
use lsp_server::{Connection, Message, Notification, Request, Response};

use crate::BoxError;

/// A server, as the main loop sees it.
pub trait Handler {
    /// What background work of the server tells it, over the channel given to [`main_loop`].
    type Event;

    /// A request of the client, other than `shutdown`, which the loop answers itself.
    fn request(&mut self, request: Request) -> Result<(), BoxError>;

    fn notification(&mut self, notification: Notification) -> Result<(), BoxError>;

    /// The answer to a request the server sent.
    fn response(&mut self, response: Response) -> Result<(), BoxError> {
        let _ = response;
        Ok(())
    }

    fn event(&mut self, event: Self::Event) -> Result<(), BoxError>;

    /// Runs once the client has nothing more queued, so that work which follows from a change
    /// (diagnostics, say) is done once per burst of keystrokes and not once per keystroke.
    fn idle(&mut self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// Runs a server on a connection whose initialization is done, until the client asks it to shut
/// down or goes away. An error of the handler ends the loop with that error.
pub fn main_loop<H: Handler>(
    connection: &Connection,
    events: &Receiver<H::Event>,
    handler: &mut H,
) -> Result<(), BoxError> {
    let mut events = events.clone();
    loop {
        select! {
            recv(connection.receiver) -> message => {
                let Ok(message) = message else {
                    return Ok(());
                };
                if handle(connection, handler, message)? {
                    return Ok(());
                }
                while let Ok(message) = connection.receiver.try_recv() {
                    if handle(connection, handler, message)? {
                        return Ok(());
                    }
                }
                handler.idle()?;
            }
            recv(events) -> event => match event {
                Ok(event) => handler.event(event)?,
                // With every sender gone the channel would be ready forever and spin the loop.
                Err(_) => events = crossbeam_channel::never(),
            }
        }
    }
}

/// Handles one message and says whether the server is done.
fn handle<H: Handler>(connection: &Connection, handler: &mut H, message: Message) -> Result<bool, BoxError> {
    match message {
        Message::Request(request) => {
            if connection.handle_shutdown(&request)? {
                return Ok(true);
            }
            handler.request(request)?;
        }
        Message::Notification(notification) => handler.notification(notification)?,
        Message::Response(response) => handler.response(response)?,
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Sender;
    use lsp_server::RequestId;
    use serde_json::{Value, json};

    /// Writes down what the loop handed it, and tells the test as it goes.
    #[derive(Default)]
    struct Recorder {
        seen: Vec<String>,
        echo: Option<Sender<String>>,
        fail_on: Option<&'static str>,
    }

    impl Recorder {
        fn saw(&mut self, line: String) {
            if let Some(echo) = &self.echo {
                let _ = echo.send(line.clone());
            }
            self.seen.push(line);
        }
    }

    impl Handler for Recorder {
        type Event = u32;

        fn request(&mut self, request: Request) -> Result<(), BoxError> {
            self.saw(format!("request {}", request.method));
            if self.fail_on == Some(request.method.as_str()) {
                return Err("failed".into());
            }
            Ok(())
        }

        fn notification(&mut self, notification: Notification) -> Result<(), BoxError> {
            self.saw(format!("notification {}", notification.method));
            Ok(())
        }

        fn response(&mut self, response: Response) -> Result<(), BoxError> {
            self.saw(format!("response {}", response.id));
            Ok(())
        }

        fn event(&mut self, event: u32) -> Result<(), BoxError> {
            self.saw(format!("event {event}"));
            Ok(())
        }

        fn idle(&mut self) -> Result<(), BoxError> {
            self.saw("idle".to_string());
            Ok(())
        }
    }

    fn request(id: i32, method: &str) -> Message {
        Request::new(RequestId::from(id), method.to_string(), Value::Null).into()
    }

    fn notification(method: &str) -> Message {
        Notification::new(method.to_string(), Value::Null).into()
    }

    fn wait_for(echo: &crossbeam_channel::Receiver<String>, line: &str) {
        while echo.recv().expect("the loop runs") != line {}
    }

    #[test]
    fn handles_everything_queued_before_it_goes_idle() {
        let (server, client) = Connection::memory();
        for message in [
            notification("textDocument/didChange"),
            notification("textDocument/didChange"),
            Response::new_ok(RequestId::from(4), json!(null)).into(),
            request(1, "textDocument/hover"),
        ] {
            client.sender.send(message).expect("sent");
        }
        let (_events_sender, events) = crossbeam_channel::unbounded::<u32>();
        let (echo_sender, echo) = crossbeam_channel::unbounded();
        let mut recorder = Recorder {
            echo: Some(echo_sender),
            ..Recorder::default()
        };
        let thread = std::thread::spawn(move || {
            main_loop(&server, &events, &mut recorder).expect("runs to the end");
            recorder.seen
        });
        wait_for(&echo, "idle");
        let shutdown_id = RequestId::from(2);
        client
            .sender
            .send(Request::new(shutdown_id.clone(), "shutdown".to_string(), Value::Null).into())
            .expect("sent");
        let answer = client.receiver.recv().expect("the shutdown is answered");
        let Message::Response(answer) = answer else {
            panic!("a response: {answer:?}");
        };
        assert_eq!(answer.id, shutdown_id);
        client.sender.send(notification("exit")).expect("sent");
        assert_eq!(
            thread.join().expect("no panic"),
            [
                "notification textDocument/didChange",
                "notification textDocument/didChange",
                "response 4",
                "request textDocument/hover",
                "idle"
            ]
        );
    }

    #[test]
    fn hands_events_of_background_work_to_the_server() {
        let (server, client) = Connection::memory();
        let (sender, events) = crossbeam_channel::unbounded();
        let (echo_sender, echo) = crossbeam_channel::unbounded();
        let mut recorder = Recorder {
            echo: Some(echo_sender),
            ..Recorder::default()
        };
        sender.send(7).expect("sent");
        sender.send(8).expect("sent");
        let thread = std::thread::spawn(move || {
            main_loop(&server, &events, &mut recorder).expect("runs to the end");
            recorder.seen
        });
        wait_for(&echo, "event 8");
        drop(client);
        assert_eq!(thread.join().expect("no panic"), ["event 7", "event 8"]);
    }

    #[test]
    fn goes_on_when_every_sender_of_events_is_gone() {
        let (server, client) = Connection::memory();
        let (sender, events) = crossbeam_channel::unbounded::<u32>();
        sender.send(1).expect("sent");
        drop(sender);
        let (echo_sender, echo) = crossbeam_channel::unbounded();
        let mut recorder = Recorder {
            echo: Some(echo_sender),
            ..Recorder::default()
        };
        let thread = std::thread::spawn(move || {
            main_loop(&server, &events, &mut recorder).expect("runs to the end");
            recorder.seen
        });
        wait_for(&echo, "event 1");
        client.sender.send(notification("initialized")).expect("sent");
        wait_for(&echo, "idle");
        drop(client);
        assert_eq!(
            thread.join().expect("no panic"),
            ["event 1", "notification initialized", "idle"]
        );
    }

    #[test]
    fn an_error_of_the_server_ends_the_loop_with_it() {
        let (server, client) = Connection::memory();
        let (_sender, events) = crossbeam_channel::unbounded::<u32>();
        let mut recorder = Recorder {
            fail_on: Some("boom"),
            ..Recorder::default()
        };
        client.sender.send(request(1, "boom")).expect("sent");
        let result = main_loop(&server, &events, &mut recorder);
        assert_eq!(result.expect_err("an error").to_string(), "failed");
    }
}
