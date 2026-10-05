//! Kernel `host_request("agent_message.send")` — nuclear-family messaging.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use super::ServerRuntime;
use super::agents::observe_target_matches_for_host;
use devo_protocol::native::ids::SessionId;

impl ServerRuntime {
    /// Kernel `host_request("agent_message.send")`.
    ///
    /// Reach is limited to the nuclear family (parent / sibling / child).
    /// `target: "all"` broadcasts to every family member except self.
    /// Child→parent injects a wake turn on the parent; peer delivery uses the
    /// agent mailbox (drains immediately when the target is idle).
    pub(crate) async fn host_agent_message_send(
        self: &Arc<Self>,
        from_session_id: &str,
        params: &serde_json::Value,
    ) -> serde_json::Value {
        let from_sid = SessionId::from_string(from_session_id.to_owned());
        let broadcast = params
            .get("target")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t.eq_ignore_ascii_case("all"));
        let message = params
            .get("message")
            .or_else(|| params.get("text"))
            .or_else(|| params.get("content"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if message.is_empty() {
            return json!({
                "status": "error",
                "error": "agent_message.send requires message"
            });
        }

        if broadcast {
            let family = self.observe_family_summaries(from_sid).await;
            let mut receipts = Vec::new();
            for agent in family {
                let relationship = agent
                    .get("relationship")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if relationship == "self" {
                    continue;
                }
                let Some(to_sid) = agent
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(|s| SessionId::from_string(s.to_owned()))
                else {
                    continue;
                };
                let name = agent
                    .get("sessionName")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                match self
                    .deliver_family_agent_message(from_sid, to_sid, relationship, &message)
                    .await
                {
                    Ok(mut receipt) => {
                        if let Some(obj) = receipt.as_object_mut() {
                            obj.insert("receiverRole".into(), relationship.into());
                            if let Some(name) = name {
                                obj.insert("receiverName".into(), name.into());
                            }
                            obj.insert("targetSessionId".into(), to_sid.to_string().into());
                        }
                        receipts.push(receipt);
                    }
                    Err(err) => {
                        receipts.push(json!({
                            "targetSessionId": to_sid.to_string(),
                            "receiverRole": relationship,
                            "receiverName": name,
                            "error": err,
                        }));
                    }
                }
            }
            super::kernel_host::push_bash_notice(
                from_session_id,
                json!({
                    "kind": "agent_message",
                    "params": params,
                }),
            );
            return json!({
                "status": "ok",
                "receipts": receipts,
            });
        }

        let role = params
            .get("receiver_role")
            .or_else(|| params.get("role"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_ascii_lowercase());
        let Some(role) = role else {
            return json!({
                "status": "error",
                "error": "agent_message.send requires receiver_role (parent|sibling|child) or target=\"all\""
            });
        };
        let receiver_name = params
            .get("receiver_name")
            .or_else(|| params.get("target"))
            .or_else(|| params.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        match role.as_str() {
            "parent" => {
                if receiver_name.is_some() {
                    return json!({
                        "status": "error",
                        "error": "receiver_name must be omitted for parent messages"
                    });
                }
                match self
                    .deliver_family_agent_message(from_sid, from_sid, "parent", &message)
                    .await
                {
                    Ok(receipt) => {
                        super::kernel_host::push_bash_notice(
                            from_session_id,
                            json!({
                                "kind": "agent_message",
                                "params": params,
                            }),
                        );
                        receipt
                    }
                    Err(err) => json!({
                        "status": "error",
                        "error": err,
                    }),
                }
            }
            "sibling" | "child" => {
                let Some(ref name) = receiver_name else {
                    return json!({
                        "status": "error",
                        "error": format!(
                            "agent_message.send to {role} requires receiver_name"
                        )
                    });
                };
                let family = self.observe_family_summaries(from_sid).await;
                let matched: Vec<_> = family
                    .into_iter()
                    .filter(|a| {
                        a.get("relationship").and_then(|v| v.as_str()) == Some(role.as_str())
                            && observe_target_matches_for_host(a, name)
                    })
                    .collect();
                if matched.len() != 1 {
                    return json!({
                        "status": "error",
                        "error": format!(
                            "agent_message.send: expected exactly one {role} matching '{name}', found {}",
                            matched.len()
                        ),
                    });
                }
                let Some(to_sid) = matched[0]
                    .get("sessionId")
                    .and_then(|v| v.as_str())
                    .map(|s| SessionId::from_string(s.to_owned()))
                else {
                    return json!({
                        "status": "error",
                        "error": "agent_message.send: matched agent missing sessionId"
                    });
                };
                match self
                    .deliver_family_agent_message(from_sid, to_sid, role.as_str(), &message)
                    .await
                {
                    Ok(mut receipt) => {
                        if let Some(obj) = receipt.as_object_mut() {
                            obj.insert("receiverRole".into(), role.clone().into());
                            obj.insert("receiverName".into(), name.clone().into());
                            obj.insert("targetSessionId".into(), to_sid.to_string().into());
                        }
                        super::kernel_host::push_bash_notice(
                            from_session_id,
                            json!({
                                "kind": "agent_message",
                                "params": params,
                            }),
                        );
                        receipt
                    }
                    Err(err) => json!({
                        "status": "error",
                        "error": err,
                    }),
                }
            }
            other => json!({
                "status": "error",
                "error": format!(
                    "receiver_role must be parent|sibling|child, got '{other}'"
                )
            }),
        }
    }

    /// Deliver one nuclear-family message. For `role == "parent"`, `to_session_id`
    /// is ignored and the sender's parent is resolved.
    async fn deliver_family_agent_message(
        self: &Arc<Self>,
        from_session_id: SessionId,
        to_session_id: SessionId,
        role: &str,
        message: &str,
    ) -> Result<serde_json::Value, String> {
        let now = Utc::now().to_rfc3339();
        if role == "parent" {
            let Some((parent_id, from_path)) = self.child_parent_and_path(from_session_id).await
            else {
                return Err("agent_message.send to parent: no parent for this session".into());
            };
            let wake = format!("Agent message received\nFrom: {from_path}\n\n{message}");
            super::kernel_host::push_bash_notice(
                parent_id.as_ref(),
                json!({
                    "kind": "agent_message",
                    "params": {
                        "fromSessionId": from_session_id.to_string(),
                        "fromAgentPath": from_path,
                        "message": message,
                    }
                }),
            );
            let busy = self.active_turn_id_for_session(parent_id).await.is_some();
            return match Arc::clone(self)
                .start_runtime_turn(
                    parent_id,
                    wake.clone(),
                    wake,
                    Some(json!({
                        "source": "agent_message",
                        "fromSessionId": from_session_id.to_string(),
                    })),
                )
                .await
            {
                Ok(runtime_turn) => {
                    let mut receipt = json!({
                        "status": "ok",
                        "delivered": true,
                        "receiverRole": "parent",
                        "parentSessionId": parent_id.to_string(),
                        "targetSessionId": parent_id.to_string(),
                        "turnId": runtime_turn.turn_id().to_string(),
                    });
                    if busy {
                        receipt["deliveryStatus"] = "queued".into();
                        receipt["queued"] = true.into();
                        receipt["queuedAt"] = now.into();
                    } else {
                        receipt["deliveryStatus"] = "delivered".into();
                        receipt["deliveredAt"] = now.into();
                    }
                    Ok(receipt)
                }
                Err(err) => Err(err.to_string()),
            };
        }

        let from_path = self.session_agent_path(from_session_id).await;
        let to_path = self.session_agent_path(to_session_id).await;
        let mailbox_message = devo_protocol::AgentMailboxMessage {
            message_id: String::new(),
            from_session_id,
            to_session_id,
            from_agent_path: from_path,
            to_agent_path: to_path,
            content: message.to_string(),
            sequence: 0,
            created_at: Utc::now(),
        };
        self.mailbox(to_session_id)
            .await
            .send(mailbox_message)
            .await
            .map_err(|error| error.to_string())?;

        let busy = self
            .active_turn_id_for_session(to_session_id)
            .await
            .is_some();
        if !busy {
            Arc::clone(self)
                .drain_child_mailbox_into_user_turns(to_session_id)
                .await
                .map_err(|error| error.to_string())?;
        }

        let mut receipt = json!({
            "status": "ok",
            "delivered": true,
            "taskId": to_session_id.to_string(),
            "receiverRole": role,
            "targetSessionId": to_session_id.to_string(),
        });
        if busy {
            receipt["deliveryStatus"] = "queued".into();
            receipt["queued"] = true.into();
            receipt["queuedAt"] = now.into();
        } else {
            receipt["deliveryStatus"] = "delivered".into();
            receipt["deliveredAt"] = now.into();
        }
        Ok(receipt)
    }
}
