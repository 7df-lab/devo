//! Serialized admission and generation-checked publication of discovery results.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::*;

#[derive(Default)]
pub(super) struct OperationSlot {
    busy: AtomicBool,
    generation: AtomicU64,
    pub(super) completed: tokio::sync::Notify,
}

impl OperationSlot {
    pub(super) fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    pub(super) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }
}

struct Reservation(Arc<OperationSlot>);
impl Drop for Reservation {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::SeqCst);
        self.0.completed.notify_waiters();
    }
}

pub(super) enum ConnectionMode {
    Reuse,
    Replace,
}

impl RmcpMcpManager {
    pub(super) async fn discover_server(
        &self,
        server_id: &McpServerId,
        mode: ConnectionMode,
    ) -> Result<(McpServerStatus, Vec<McpToolInfo>), McpError> {
        let slot = self.operations.get(server_id).cloned().ok_or_else(|| {
            McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            }
        })?;
        let mut joined_discovery = false;
        let (record, generation, _reservation, previous) = loop {
            // Register before checking busy so completion/cancellation cannot
            // race the waiter between dropping the gate and awaiting notification.
            let completed = slot.completed.notified();
            tokio::pin!(completed);
            completed.as_mut().enable();
            let _gate = self.operation_gate.lock().await;
            let record = self.record(server_id).await?;
            if !record.enabled {
                return Err(McpError::McpServerUnavailable {
                    server_id: server_id.clone(),
                });
            }
            if slot.is_busy() {
                if matches!(mode, ConnectionMode::Replace) {
                    return Err(McpError::McpProtocolError {
                        server_id: server_id.clone(),
                        message: "MCP discovery or reconnect is already in progress".into(),
                    });
                }
                drop(_gate);
                completed.await;
                joined_discovery = true;
                continue;
            }
            if joined_discovery {
                let status = self.statuses.read().await[server_id].clone();
                if status.startup_state == McpStartupState::Ready {
                    let tools = self
                        .tool_cache
                        .read()
                        .await
                        .get(server_id)
                        .cloned()
                        .unwrap_or_default();
                    return Ok((status, tools));
                }
                if status.startup_state == McpStartupState::Failed {
                    return Err(McpError::McpProtocolError {
                        server_id: server_id.clone(),
                        message: "The concurrent MCP discovery failed; retry explicitly".into(),
                    });
                }
            }
            slot.busy.store(true, Ordering::SeqCst);
            let reservation = Reservation(Arc::clone(&slot));
            let generation = slot.generation.load(Ordering::SeqCst);
            let previous = match mode {
                ConnectionMode::Reuse => self.clients.read().await.get(server_id).cloned(),
                ConnectionMode::Replace => {
                    self.clients.write().await.remove(server_id);
                    None
                }
            };
            self.tool_cache.write().await.remove(server_id);
            self.statuses.write().await.insert(
                server_id.clone(),
                empty_status(server_id.clone(), McpStartupState::Starting),
            );
            break (record, generation, reservation, previous);
        };

        // No state lock is held across process startup, initialization or tools/list.
        let discovered = async {
            let client = match previous {
                Some(client) => client,
                None => Arc::new(create_client(&record, self.oauth_store_mode).await?),
            };
            let mut tools = Vec::new();
            if tools_capability_allowed(&record) {
                let list = client
                    .list_tools_with_connector_ids(None, Some(MCP_OPERATION_TIMEOUT))
                    .await
                    .map_err(|err| McpError::McpProtocolError {
                        server_id: server_id.clone(),
                        message: err.to_string(),
                    })?;
                tools = list
                    .tools
                    .into_iter()
                    .map(|tool| {
                        mcp_tool_info_from_rmcp_tool(
                            &record,
                            tool.tool,
                            tool.connector_description,
                            /*supports_parallel_tool_calls*/ false,
                        )
                    })
                    .collect();
            }
            Ok::<_, McpError>((client, tools))
        }
        .await;

        let _gate = self.operation_gate.lock().await;
        if slot.generation.load(Ordering::SeqCst) != generation
            || !self.record(server_id).await?.enabled
        {
            return Err(McpError::McpServerUnavailable {
                server_id: server_id.clone(),
            });
        }
        match discovered {
            Ok((client, tools)) => {
                let mut status = empty_status(server_id.clone(), McpStartupState::Ready);
                status.auth_state = McpAuthState::Authenticated;
                status.last_refreshed_at = Some(chrono::Utc::now());
                status.tools = tools
                    .iter()
                    .map(|tool| McpToolDescriptor {
                        server_id: server_id.clone(),
                        name: tool.raw_tool_name.clone(),
                        description: tool.description.clone().unwrap_or_default(),
                        input_schema: tool.input_schema.clone(),
                    })
                    .collect();
                self.clients.write().await.insert(server_id.clone(), client);
                self.tool_cache
                    .write()
                    .await
                    .insert(server_id.clone(), tools.clone());
                self.statuses
                    .write()
                    .await
                    .insert(server_id.clone(), status.clone());
                Ok((status, tools))
            }
            Err(err) => {
                self.clients.write().await.remove(server_id);
                let mut status = empty_status(server_id.clone(), McpStartupState::Failed);
                status.last_refreshed_at = Some(chrono::Utc::now());
                self.statuses
                    .write()
                    .await
                    .insert(server_id.clone(), status);
                Err(err)
            }
        }
    }
}

#[cfg(test)]
#[path = "reconnect_tests.rs"]
mod tests;
