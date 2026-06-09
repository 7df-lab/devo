use super::*;
use crate::PendingServerRequestContext;
use crate::ServerRequestKind;

impl ServerRuntime {
    pub(super) async fn handle_request_user_input_respond(
        &self,
        request_id: serde_json::Value,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let params: RequestUserInputRespondParams = match serde_json::from_value(params) {
            Ok(params) => params,
            Err(error) => {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    format!("invalid request_user_input/respond params: {error}"),
                );
            }
        };

        let Some(session_arc) = self.sessions.lock().await.get(&params.session_id).cloned() else {
            return self.error_response(
                request_id,
                ProtocolErrorCode::SessionNotFound,
                "session does not exist",
            );
        };

        let request_key = params.request_id.to_string();
        let pending = {
            let mut session = session_arc.lock().await;
            let Some(pending) = session.pending_user_inputs.remove(&request_key) else {
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "no pending request_user_input request exists for this runtime",
                );
            };
            if pending.turn_id != params.turn_id {
                session.pending_user_inputs.insert(request_key, pending);
                return self.error_response(
                    request_id,
                    ProtocolErrorCode::InvalidParams,
                    "request_user_input belongs to a different turn",
                );
            }
            pending
        };

        let _ = pending.tx.send(params.response);
        self.broadcast_event(ServerEvent::ServerRequestResolved(
            ServerRequestResolvedPayload {
                session_id: params.session_id,
                request_id: params.request_id.clone(),
                turn_id: Some(params.turn_id),
            },
        ))
        .await;

        serde_json::to_value(SuccessResponse {
            id: request_id,
            result: serde_json::json!({ "request_id": params.request_id }),
        })
        .expect("serialize request_user_input response")
    }

    pub(super) async fn request_user_input_for_tool(
        &self,
        session_id: SessionId,
        turn_id: TurnId,
        tool_call_id: String,
        args: RequestUserInputArgs,
    ) -> Result<RequestUserInputResponse, ToolCallError> {
        let request_id = tool_call_id;
        let (tx, rx) = oneshot::channel();

        let Some(session_arc) = self.sessions.lock().await.get(&session_id).cloned() else {
            return Err(ToolCallError::ExecutionFailed(
                "session does not exist".to_string(),
            ));
        };
        {
            let mut session = session_arc.lock().await;
            if session
                .pending_user_inputs
                .insert(request_id.clone(), PendingUserInput { turn_id, tx })
                .is_some()
            {
                tracing::warn!(
                    session_id = %session_id,
                    turn_id = %turn_id,
                    request_id = %request_id,
                    "overwriting pending request_user_input request"
                );
            }
        }

        self.broadcast_event(ServerEvent::RequestUserInput(RequestUserInputPayload {
            request: PendingServerRequestContext {
                request_id: request_id.clone().into(),
                request_kind: ServerRequestKind::ItemToolRequestUserInput,
                session_id,
                turn_id: Some(turn_id),
                item_id: None,
            },
            questions: args.questions,
        }))
        .await;

        rx.await.map_err(|_| {
            ToolCallError::ExecutionFailed("request_user_input channel closed".to_string())
        })
    }
}
