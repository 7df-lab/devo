//! Provider authentication notifications.

use devo_protocol::native::event::ServerNotification;

use super::ServerRuntime;
use super::outbound::{OutboundDeliveryPolicy, OutboundFrame, enqueue_outbound_notification};

impl ServerRuntime {
    pub(crate) async fn notify_provider_auth_stale(
        &self,
        provider_id: impl Into<String>,
        reason: Option<String>,
    ) {
        let notification = ServerNotification::ProviderAuthStale {
            provider_id: provider_id.into(),
            reason,
        };
        let value = serde_json::to_value(notification).expect("provider auth stale notification");
        let method = value["method"]
            .as_str()
            .expect("notification method")
            .to_string();
        let params = value["params"].clone();
        let recipients = {
            let mut connections = self.connections.lock().await;
            connections
                .iter_mut()
                .filter_map(|(id, connection)| {
                    if connection.protocol != Some(super::connection::ConnectionProtocol::Native) {
                        return None;
                    }
                    Some((
                        connection.outbound_tx.clone(),
                        OutboundFrame::notification(
                            *id,
                            method.clone(),
                            connection.next_seq(),
                            params.clone(),
                        ),
                    ))
                })
                .collect::<Vec<_>>()
        };
        for (sender, frame) in recipients {
            let _ = enqueue_outbound_notification(
                &sender,
                frame,
                OutboundDeliveryPolicy::Reliable,
                "provider_auth_stale",
            )
            .await;
        }
    }
}
