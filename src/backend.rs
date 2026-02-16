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
        match task {
            BackendTask::RefreshAll => {
                let c1 = client.clone();
                let status = tokio::task::spawn_blocking(move || c1.status()).await;
                let c2 = client.clone();
                let managed = tokio::task::spawn_blocking(move || c2.managed()).await;
                let c3 = client.clone();
                let unmanaged = tokio::task::spawn_blocking(move || c3.unmanaged()).await;

                match (status, managed, unmanaged) {
                    (Ok(Ok(status)), Ok(Ok(managed)), Ok(Ok(unmanaged))) => {
                        if event_tx
                            .send(BackendEvent::Refreshed {
                                status,
                                managed,
                                unmanaged,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    (s, m, u) => {
                        let message = format!(
                            "refresh failed: status={:?}, managed={:?}, unmanaged={:?}",
                            flatten_error(s),
                            flatten_error(m),
                            flatten_error(u)
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
            BackendTask::LoadDiff { target } => {
                let c = client.clone();
                let target_for_worker = target.clone();
                let result =
                    tokio::task::spawn_blocking(move || c.diff(target_for_worker.as_deref())).await;
                match result {
                    Ok(Ok(diff)) => {
                        if event_tx
                            .send(BackendEvent::DiffLoaded { target, diff })
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
            BackendTask::LoadPreview { target, absolute } => {
                let result =
                    tokio::task::spawn_blocking(move || load_file_preview(&absolute)).await;
                match result {
                    Ok(Ok(content)) => {
                        if event_tx
                            .send(BackendEvent::PreviewLoaded { target, content })
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
