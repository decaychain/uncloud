use std::sync::Arc;
use std::time::Duration;

use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::config::{ManagedApp, RestartPolicy};
use crate::AppState;

pub struct Supervisor {
    state: Arc<AppState>,
    pub shutdown: CancellationToken,
}

impl Supervisor {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            shutdown: CancellationToken::new(),
        }
    }

    pub async fn start_all(&self) {
        for app in &self.state.config.apps.managed {
            let app = app.clone();
            let shutdown = self.shutdown.clone();
            tokio::spawn(async move {
                supervise(app, shutdown).await;
            });
        }
    }
}

async fn supervise(app: ManagedApp, shutdown: CancellationToken) {
    let mut attempts = 0u32;

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let (program, args) = app.command.parts();
        tracing::info!("[{}] Starting: {}", app.name, app.command.display());

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .envs(&app.env)
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);

        // Put the child in its own process group so that SIGTERM reaches the
        // entire process tree (e.g. `cargo run` + the binary it spawns).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe { cmd.pre_exec(|| { libc::setpgid(0, 0); Ok(()) }); }
        }

        if let Some(dir) = &app.working_dir {
            cmd.current_dir(dir);
        }

        let result = match cmd.spawn() {
            Ok(mut child) => {
                // Capture the PGID before entering the select — child.id() is
                // only valid while the process is alive.
                #[cfg(unix)]
                let pgid = child.id().map(|pid| pid as libc::pid_t);

                tokio::select! {
                    status = child.wait() => status,
                    _ = shutdown.cancelled() => {
                        tracing::info!("[{}] Shutdown signal received, stopping", app.name);
                        // Send SIGTERM to the whole process group so grandchildren
                        // (e.g. the binary spawned by `cargo run`) also get the signal.
                        #[cfg(unix)]
                        if let Some(pgid) = pgid {
                            unsafe { libc::kill(-pgid, libc::SIGTERM); }
                        }
                        // kill_on_drop will SIGKILL the direct child if it hasn't
                        // exited by the time `child` is dropped here.
                        return;
                    }
                }
            }
            Err(e) => {
                tracing::error!("[{}] Failed to spawn: {}", app.name, e);
                Err(e)
            }
        };

        let should_restart = match &result {
            Ok(status) if status.success() => {
                tracing::info!("[{}] Exited cleanly (code 0)", app.name);
                app.restart == RestartPolicy::Always
            }
            Ok(status) => {
                tracing::warn!("[{}] Exited with status {}", app.name, status);
                app.restart != RestartPolicy::Never
            }
            Err(_) => app.restart != RestartPolicy::Never,
        };

        if !should_restart {
            break;
        }

        attempts += 1;
        if app.restart_max_attempts > 0 && attempts >= app.restart_max_attempts {
            tracing::error!(
                "[{}] Reached max restart attempts ({}), giving up",
                app.name,
                app.restart_max_attempts
            );
            break;
        }

        let backoff = (app.restart_backoff_secs * 2u64.saturating_pow(attempts - 1)).min(60);
        tracing::info!(
            "[{}] Restarting in {}s (attempt {})",
            app.name,
            backoff,
            attempts + 1
        );

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            _ = shutdown.cancelled() => {
                tracing::info!("[{}] Shutdown during backoff, stopping", app.name);
                return;
            }
        }
    }
}
