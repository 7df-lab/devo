use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::UnboundedSender;

use crate::app_command::AppCommand;
use crate::app_event::AppEvent;

#[derive(Clone, Debug)]
pub(crate) struct AppEventSender {
    app_event_tx: AppEventTx,
}

#[derive(Clone, Debug)]
enum AppEventTx {
    Bounded(Sender<AppEvent>),
    Unbounded(UnboundedSender<AppEvent>),
}

impl AppEventSender {
    pub(crate) fn new(app_event_tx: UnboundedSender<AppEvent>) -> Self {
        Self {
            app_event_tx: AppEventTx::Unbounded(app_event_tx),
        }
    }

    pub(crate) fn new_bounded(app_event_tx: Sender<AppEvent>) -> Self {
        Self {
            app_event_tx: AppEventTx::Bounded(app_event_tx),
        }
    }

    /// Send an event to the app event channel. If the receiver has gone away,
    /// the UI is shutting down, so logging the failure is enough.
    pub(crate) fn send(&self, event: AppEvent) {
        match &self.app_event_tx {
            AppEventTx::Bounded(tx) => {
                if let Err(err) = tx.try_send(event) {
                    tracing::error!("failed to send v2 app event: {err}");
                }
            }
            AppEventTx::Unbounded(tx) => {
                if let Err(err) = tx.send(event) {
                    tracing::error!("failed to send v2 app event: {err}");
                }
            }
        }
    }

    #[allow(dead_code)]
    pub(crate) fn redraw(&self) {
        self.send(AppEvent::Redraw);
    }

    #[allow(dead_code)]
    pub(crate) fn submit_user_input(&self, text: String) {
        self.send(AppEvent::SubmitUserInput { text });
    }

    #[allow(dead_code)]
    pub(crate) fn user_input_answer(&self, id: String, response: String) {
        self.send(AppEvent::Command(AppCommand::RunUserShellCommand {
            command: format!("user_input_answer {id} {response}"),
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn exec_approval(&self, _thread_id: String, id: String, decision: String) {
        self.send(AppEvent::Command(AppCommand::RunUserShellCommand {
            command: format!("exec_approval {id} {decision}"),
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn request_permissions_response(
        &self,
        _thread_id: String,
        id: String,
        response: String,
    ) {
        self.send(AppEvent::Command(AppCommand::RunUserShellCommand {
            command: format!("request_permissions_response {id} {response}"),
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn patch_approval(&self, _thread_id: String, id: String, decision: String) {
        self.send(AppEvent::Command(AppCommand::RunUserShellCommand {
            command: format!("patch_approval {id} {decision}"),
        }));
    }

    #[allow(dead_code)]
    pub(crate) fn resolve_elicitation(
        &self,
        _thread_id: String,
        server_name: String,
        request_id: String,
        decision: String,
        content: Option<serde_json::Value>,
        meta: Option<serde_json::Value>,
    ) {
        let payload = serde_json::json!({
            "server_name": server_name,
            "request_id": request_id,
            "decision": decision,
            "content": content,
            "meta": meta,
        });
        self.send(AppEvent::Command(AppCommand::RunUserShellCommand {
            command: format!("resolve_elicitation {payload}"),
        }));
    }
}
