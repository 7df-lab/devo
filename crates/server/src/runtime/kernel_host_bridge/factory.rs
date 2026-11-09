//! Host-bound Factory checkpoints. Reads never create files or hydrate children.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    HostBridge, check_or_error, deny_plan_mutation, error_reply, permission_request,
    plan_mode_denies_mutation, synthetic_tool_call_id,
};
use devo_safety::ResourceKind;

mod effects;
mod files;
mod store;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveParams {
    run_id: String,
    expected_revision: u64,
    snapshot: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadParams {
    run_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {}

enum Operation {
    Save(SaveParams),
    Load(LoadParams),
    List,
    Effect(effects::Params),
}

pub(crate) async fn handle_factory(bridge: &HostBridge, action: &str, params: &Value) -> Value {
    if matches!(action, "factory.save" | "factory.spawn" | "factory.cancel")
        && plan_mode_denies_mutation(bridge)
    {
        return deny_plan_mutation(action);
    }
    let parsed = match action {
        "factory.save" => serde_json::from_value(params.clone()).map(Operation::Save),
        "factory.load" => serde_json::from_value(params.clone()).map(Operation::Load),
        "factory.spawn" | "factory.cancel" => {
            serde_json::from_value(params.clone()).map(Operation::Effect)
        }
        "factory.list" => {
            serde_json::from_value::<ListParams>(params.clone()).map(|_| Operation::List)
        }
        _ => return error_reply(format!("unknown Factory action: {action}")),
    };
    let operation = match parsed {
        Ok(operation) => operation,
        Err(error) => return error_reply(format!("invalid {action} request: {error}")),
    };
    let validation = match &operation {
        Operation::Save(params) => store::validate_id(&params.run_id)
            .and_then(|_| store::validate_snapshot(&params.run_id, &params.snapshot)),
        Operation::Load(params) => store::validate_id(&params.run_id),
        Operation::List => Ok(()),
        Operation::Effect(params) => effects::validate(action, params),
    };
    if let Err(error) = validation {
        return error_reply(error.to_string());
    }
    let Some(root) = &bridge.session_dir else {
        return match operation {
            Operation::List => json!({"status":"ok","result":{"runs":[]}}),
            Operation::Load(params) => {
                error_reply(format!("Factory run not found: {}", params.run_id))
            }
            Operation::Save(_) | Operation::Effect(_) => {
                error_reply("This session has no persistent Factory store")
            }
        };
    };
    // Open the host-owned directory before permission waits. On Unix every
    // subsequent operation is descriptor-relative and refuses symlinks.
    let root = match files::Directory::open(root) {
        Ok(root) => root,
        Err(error) => return error_reply(format!("open Factory session directory: {error}")),
    };
    let directory = match root.child("factory") {
        Ok(directory) => Some(directory),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return error_reply(format!("open Factory store: {error}")),
    };
    let mutation = matches!(operation, Operation::Save(_) | Operation::Effect(_));
    // Approval covers the store directory, including its stable lock and
    // unique temporary files. No unapproved sidecar path is opened for write.
    let path: PathBuf = root.path.join("factory");
    let request = permission_request(
        bridge,
        synthetic_tool_call_id(action),
        if mutation { "write" } else { "read" },
        params.clone(),
        if mutation {
            ResourceKind::FileWrite
        } else {
            ResourceKind::FileRead
        },
        format!(
            "{action} {} (Factory snapshots and transaction sidecars)",
            path.display()
        ),
        Some(path),
        /*target*/ None,
    );
    if let Err(reply) = check_or_error(bridge, request).await {
        return reply;
    }
    if bridge.cancel_token.is_cancelled() {
        return error_reply("cancelled");
    }
    if let Operation::Effect(params) = operation {
        let Some(directory) = directory else {
            return error_reply("Factory effect requires a saved admission intent");
        };
        return effects::execute(bridge, directory, action, params).await;
    }
    let owner = bridge.session_id;
    match tokio::task::spawn_blocking(move || {
        let directory = match (directory, &operation) {
            (Some(directory), _) => directory,
            (None, Operation::Save(_)) => root.create_child("factory")?,
            (None, Operation::List) => return Ok(json!({"runs":[]})),
            (None, Operation::Effect(_)) => unreachable!("effects dispatched above"),
            (None, Operation::Load(params)) => {
                anyhow::bail!("Factory run not found: {}", params.run_id)
            }
        };
        match operation {
            Operation::Save(params) => store::save(
                &directory,
                owner,
                params.run_id,
                params.expected_revision,
                params.snapshot,
            ),
            Operation::Load(params) => {
                store::load(&directory, owner, &params.run_id).map(|run| run.response())
            }
            Operation::List => store::list(&directory, owner),
            Operation::Effect(_) => unreachable!("effects dispatched above"),
        }
    })
    .await
    {
        Ok(Ok(result)) => json!({"status":"ok","result":result}),
        Ok(Err(error)) => error_reply(format!("{error:#}")),
        Err(error) => error_reply(format!("Factory persistence worker failed: {error}")),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod effects_tests;
