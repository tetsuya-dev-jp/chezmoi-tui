use crate::app::{BackendEvent, BackendTask};
use crate::infra::ChezmoiClient;
use crate::preview::load_file_preview;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub(crate) async fn worker_loop(
    client: std::sync::Arc<dyn ChezmoiClient>,
    mut task_rx: UnboundedReceiver<BackendTask>,
    event_tx: UnboundedSender<BackendEvent>,
) {
    while let Some(task) = task_rx.recv().await {
        tracing::debug!(task = ?task, "backend task received");
        match task {
            BackendTask::RefreshAll => {
                let c1 = client.clone();
                let status_task = tokio::task::spawn_blocking(move || c1.status());
                let c2 = client.clone();
                let managed_task = tokio::task::spawn_blocking(move || c2.managed());
                let c3 = client.clone();
                let unmanaged_task = tokio::task::spawn_blocking(move || c3.unmanaged());
                let c4 = client.clone();
                let source_task = tokio::task::spawn_blocking(move || c4.source());
                let (status, managed, unmanaged, source) =
                    tokio::join!(status_task, managed_task, unmanaged_task, source_task);

                match (status, managed, unmanaged, source) {
                    (
                        Ok(Ok(status)),
                        Ok(Ok(managed)),
                        Ok(Ok(unmanaged)),
                        Ok(Ok((source_dir, source))),
                    ) => {
                        tracing::info!(
                            status = status.len(),
                            managed = managed.len(),
                            unmanaged = unmanaged.len(),
                            source = source.len(),
                            source_dir = %source_dir.display(),
                            "refresh completed"
                        );
                        if event_tx
                            .send(BackendEvent::Refreshed {
                                status,
                                managed,
                                unmanaged,
                                source_dir: Some(source_dir),
                                source,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    (s, m, u, src) => {
                        let message = format!(
                            "refresh failed: status={:?}, managed={:?}, unmanaged={:?}, source={:?}",
                            flatten_error(s),
                            flatten_error(m),
                            flatten_error(u),
                            flatten_error(src)
                        );
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "refresh".to_string(),
                                message,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::LoadDiff { request_id, target } => {
                let c = client.clone();
                let target_for_worker = target.clone();
                let result =
                    tokio::task::spawn_blocking(move || c.diff(target_for_worker.as_deref())).await;
                match result {
                    Ok(Ok(diff)) => {
                        if event_tx
                            .send(BackendEvent::DiffLoaded {
                                request_id,
                                target,
                                diff,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "diff".to_string(),
                                message: format!("diff failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::LoadPreview {
                request_id,
                target,
                absolute,
                origin,
            } => {
                let result =
                    tokio::task::spawn_blocking(move || load_file_preview(&absolute)).await;
                match result {
                    Ok(Ok(content)) => {
                        if event_tx
                            .send(BackendEvent::PreviewLoaded {
                                request_id,
                                target,
                                origin,
                                content,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "preview".to_string(),
                                message: format!("preview failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::PrepareApplyPlan {
                request,
                expected_snapshot,
                mode,
            } => {
                let c = client.clone();
                let target = request.target.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let status = c.status()?;
                    let diff = c.diff(target.as_deref())?;
                    Ok::<_, anyhow::Error>((status, diff.text))
                })
                .await;

                match result {
                    Ok(Ok((status, diff_text))) => {
                        if event_tx
                            .send(BackendEvent::ApplyPlanPrepared {
                                request,
                                status,
                                diff_fingerprint: fingerprint_text(&diff_text),
                                expected_snapshot,
                                mode,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "apply-plan".to_string(),
                                message: format!(
                                    "apply plan refresh failed: {:?}",
                                    flatten_error(other)
                                ),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
            BackendTask::RunAction { request } => {
                let c = client.clone();
                let req = request.clone();
                let result = tokio::task::spawn_blocking(move || c.run(&req)).await;
                match result {
                    Ok(Ok(result)) => {
                        if event_tx
                            .send(BackendEvent::ActionFinished { request, result })
                            .is_err()
                        {
                            break;
                        }
                    }
                    other => {
                        if event_tx
                            .send(BackendEvent::Error {
                                context: "action".to_string(),
                                message: format!("action failed: {:?}", flatten_error(other)),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }
    }
}

fn flatten_error<T>(res: std::result::Result<anyhow::Result<T>, tokio::task::JoinError>) -> String {
    match res {
        Ok(Ok(_)) => "ok".to_string(),
        Ok(Err(err)) => format!("{err:#}"),
        Err(err) => format!("join error: {err}"),
    }
}

fn fingerprint_text(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_error_formats_all_cases() {
        let ok = flatten_error::<()>(Ok(Ok(())));
        assert_eq!(ok, "ok");

        let err = flatten_error::<()>(Ok(Err(anyhow::anyhow!("boom"))));
        assert!(err.contains("boom"));
    }
}
