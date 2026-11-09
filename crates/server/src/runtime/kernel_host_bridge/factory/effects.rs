//! Atomic ownership fence around host child admission and cleanup.
//!
//! A checkpoint CAS alone cannot fence a later RPC. Hold the store transaction
//! through the existing lifecycle operation; claim/save use that same OS lock.
//! Reserve a new revision before the effect so a duplicate or lost reply cannot
//! repeat an operation with the old revision. Recovery reconciles its saved name.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{HostBridge, error_reply, files::Directory, store};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Spawn,
    Cancel,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Params {
    pub run_id: String,
    expected_revision: u64,
    node_id: String,
    prompt: Option<String>,
}

pub(super) fn validate(action: &str, params: &Params) -> Result<()> {
    store::validate_id(&params.run_id)?;
    ensure!(
        !params.node_id.is_empty() && params.node_id.len() <= 32,
        "invalid Factory node id"
    );
    match action {
        "factory.spawn" => ensure!(
            params.prompt.as_ref().is_some_and(
                |prompt| !prompt.trim().is_empty() && prompt.chars().count() <= 128_000
            ),
            "Factory spawn requires a bounded prompt"
        ),
        "factory.cancel" => ensure!(
            params.prompt.is_none(),
            "Factory cancel does not accept a prompt"
        ),
        _ => anyhow::bail!("unknown Factory effect"),
    }
    Ok(())
}

pub(super) async fn execute(
    bridge: &HostBridge,
    directory: Directory,
    action: &str,
    params: Params,
) -> Value {
    let owner = bridge.session_id;
    let kind = if action == "factory.spawn" {
        Kind::Spawn
    } else {
        Kind::Cancel
    };
    let runtime = bridge.runtime();
    let cancel_token = bridge.cancel_token.clone();
    match run_fenced_effect(
        directory,
        owner,
        kind,
        params,
        cancel_token,
        move |target, kwargs, prompt| async move {
            if kind == Kind::Spawn {
                crate::runtime::kernel_host_models::spawn_with_runtime(
                    runtime,
                    owner,
                    &json!({"prompt":prompt, "kwargs":kwargs}),
                )
                .await
            } else {
                match runtime {
                    Some(runtime) => {
                        runtime
                            .host_rlm_delete_subagent(owner.as_str(), &json!({"target":target}))
                            .await
                    }
                    None => error_reply("runtime not available for Factory cleanup"),
                }
            }
        },
    )
    .await
    {
        Ok(result) => result,
        Err(error) => error_reply(format!(
            "Factory effect task failed; restore before continuing: {error}"
        )),
    }
}

// Own the whole transaction in a host task, independently of the response
// future. The callback is the existing child lifecycle path in production.
pub(super) fn run_fenced_effect<Effect, EffectFuture>(
    directory: Directory,
    owner: devo_protocol::native::ids::SessionId,
    kind: Kind,
    params: Params,
    cancel_token: tokio_util::sync::CancellationToken,
    effect: Effect,
) -> tokio::task::JoinHandle<Value>
where
    Effect: FnOnce(String, Value, Option<String>) -> EffectFuture + Send + 'static,
    EffectFuture: std::future::Future<Output = Value> + Send + 'static,
{
    // The task owns the fence, even if the kernel drops the response future.
    // Lifecycle admission may itself launch work before returning its reply.
    tokio::spawn(async move {
        let prepared = tokio::task::spawn_blocking(move || {
            let transaction = store::EffectTransaction::open(directory)?;
            let run = transaction.load(owner, &params.run_id)?;
            ensure!(
                run.revision == params.expected_revision,
                "Factory revision conflict before effect"
            );
            let node = &run.snapshot["run"]["nodes"][&params.node_id];
            ensure!(
                node.is_object() && node["needs_cleanup"] == true,
                "Factory node has no unresolved admission"
            );
            let expected_name = format!("factory-{}-{}", &params.run_id[..16], params.node_id);
            ensure!(
                node["name"] == expected_name,
                "Factory node has an invalid reserved name"
            );
            let target = node["child_id"]
                .as_str()
                .unwrap_or(&expected_name)
                .to_owned();
            let mut kwargs = json!({"name":expected_name});
            if kind == Kind::Spawn {
                ensure!(
                    run.snapshot["run"]["stop_requested"] == false
                        && node["status"] == "admitting"
                        && node["child_id"].is_null(),
                    "Factory node is not eligible for admission"
                );
                let spec = run.snapshot["spec"]["nodes"]
                    .as_array()
                    .context("invalid Factory definition")?
                    .iter()
                    .find(|node| node["id"] == params.node_id)
                    .context("Factory node is not defined")?;
                for key in ["model", "thinking"] {
                    if let Some(value) = spec.get(key) {
                        kwargs[key] = value.clone();
                    }
                }
            } else {
                ensure!(
                    run.snapshot["run"]["stop_requested"] == true,
                    "Factory cleanup requires stop intent"
                );
            }
            // Persist before the external mutation and keep the lock until it ends.
            let reserved = transaction.reserve(owner, run)?;
            Ok::<_, anyhow::Error>((transaction, reserved, target, kwargs, params.prompt))
        })
        .await;
        let (transaction, reserved, target, kwargs, prompt) = match prepared {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(error)) => return error_reply(format!("{error:#}")),
            Err(error) => {
                return error_reply(format!("Factory effect preparation failed: {error}"));
            }
        };
        if cancel_token.is_cancelled() {
            return error_reply(
                "Factory effect cancelled after reservation; restore before continuing",
            );
        }
        let response = effect(target, kwargs, prompt).await;
        // Child admission/close has returned. No future effect can outlive this fence.
        drop(transaction);
        let mut result = json!({"run_id":reserved["run_id"],"revision":reserved["revision"]});
        if response["status"] == "ok" {
            if kind == Kind::Spawn {
                result["child_id"] = response["result"]["rlm_child_id"].clone();
            }
        } else {
            return response;
        }
        json!({"status":"ok","result":result})
    })
}
