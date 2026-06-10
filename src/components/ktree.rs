use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};
use strum::Display;
use tokio::{process::Command, sync::mpsc::UnboundedSender};
use tracing::{error, info};

use super::Component;
use crate::action::{Action, KtreeStatus};
use crate::components::SPINNER;

const MAX_NOTIFICATION_TICKS: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum LocalMode {
    Idle,
    Applying,
    Success,
    Failed,
    Aborting,
}

pub struct Ktree {
    command_tx: Option<UnboundedSender<Action>>,
    local_mode: LocalMode,
    status_message: String,
    kernel_tree_path: PathBuf,
    spinner: usize,
    notification_ticks: usize,
    patch_apply_in_progress: bool,
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
}

impl Ktree {
    pub fn new(kernel_tree: String) -> Self {
        Self {
            command_tx: None,
            local_mode: LocalMode::Idle,
            status_message: String::new(),
            kernel_tree_path: expand_tilde(&kernel_tree),
            spinner: 0,
            notification_ticks: 0,
            patch_apply_in_progress: false,
        }
    }

    fn apply_patch(&self, mbox_path: String) {
        let tx = self.command_tx.clone().unwrap();
        let tree_path = self.kernel_tree_path.clone();

        tokio::spawn(async move {
            let output = Command::new("git")
                .args(["am", &mbox_path])
                .current_dir(&tree_path)
                .output()
                .await;

            match output {
                Ok(out) if out.status.success() => {
                    let log = Command::new("git")
                        .args(["log", "-1", "--format=%s"])
                        .current_dir(&tree_path)
                        .output()
                        .await;
                    let subject = log
                        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
                        .unwrap_or_else(|_| "applied".to_string());
                    info!("Ktree apply success: {subject}");
                    tx.send(Action::KtreeResult(KtreeStatus::Success(subject)))
                        .ok();
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    error!("Ktree git am failed:\n{stderr}");
                    tx.send(Action::KtreeResult(KtreeStatus::Failed(stderr)))
                        .ok();
                }
                Err(e) => {
                    error!("Ktree git am execution failed: {e}");
                    tx.send(Action::KtreeResult(KtreeStatus::Failed(format!(
                        "execution: {e}"
                    ))))
                    .ok();
                }
            }

            let _ = std::fs::remove_file(&mbox_path);
        });
    }

    fn abort_apply(&self) {
        let tx = self.command_tx.clone().unwrap();
        let tree_path = self.kernel_tree_path.clone();

        tokio::spawn(async move {
            tx.send(Action::KtreeSetMode(LocalMode::Aborting)).ok();

            // Best effort — if there's nothing to abort, that's fine
            let _ = Command::new("git")
                .args(["am", "--abort"])
                .current_dir(&tree_path)
                .output()
                .await;

            tx.send(Action::KtreeResult(KtreeStatus::Aborted)).ok();
        });
    }
}

impl Component for Ktree {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        let action = match key.code {
            KeyCode::Char('x') if self.local_mode == LocalMode::Failed => Some(Action::KtreeAbort),
            _ => None,
        };
        Ok(action)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::Tick => match self.local_mode {
                LocalMode::Success => {
                    self.notification_ticks += 1;
                    if self.notification_ticks >= MAX_NOTIFICATION_TICKS {
                        self.notification_ticks = 0;
                        self.local_mode = LocalMode::Idle;
                        self.status_message.clear();
                    }
                }
                LocalMode::Applying | LocalMode::Aborting => {
                    self.spinner = (self.spinner + 1) % SPINNER.len();
                }
                _ => {}
            },
            Action::KtreeApply(mbox_path) => {
                if self.local_mode == LocalMode::Idle || self.local_mode == LocalMode::Applying {
                    self.local_mode = LocalMode::Applying;
                    self.patch_apply_in_progress = true;
                    self.apply_patch(mbox_path);
                }
            }
            Action::KtreeSetMode(mode) => {
                self.local_mode = mode;
                self.spinner = 0;
            }
            Action::KtreeResult(outcome) => {
                self.notification_ticks = 0;
                match outcome {
                    KtreeStatus::Success(subject) => {
                        self.local_mode = LocalMode::Success;
                        self.status_message = subject;
                        self.patch_apply_in_progress = false;
                    }
                    KtreeStatus::Failed(err) => {
                        self.local_mode = LocalMode::Failed;
                        self.status_message = err;
                    }
                    KtreeStatus::Aborted => {
                        self.local_mode = LocalMode::Success;
                        self.patch_apply_in_progress = false;
                        self.notification_ticks = 0;
                        self.status_message = "git am --abort: patch reverted".to_string();
                    }
                }
            }
            Action::KtreeAbort if self.local_mode == LocalMode::Failed => {
                if self.patch_apply_in_progress {
                    self.abort_apply();
                } else {
                    self.local_mode = LocalMode::Idle;
                    self.status_message.clear();
                }
            }
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        // Only draw when there's something to show
        if self.local_mode == LocalMode::Idle && self.status_message.is_empty() {
            return Ok(());
        }

        let (text, style, title) = match &self.local_mode {
            LocalMode::Idle => (
                self.status_message.clone(),
                Style::default().fg(Color::DarkGray),
                " ktree ",
            ),
            LocalMode::Applying => (
                format!("Applying{}", SPINNER[self.spinner]),
                Style::default().fg(Color::Yellow),
                " ktree ",
            ),
            LocalMode::Success => (
                format!("[ok] {}", self.status_message),
                Style::default().fg(Color::Green),
                " ktree ",
            ),
            LocalMode::Failed => (
                self.status_message.clone(),
                Style::default().fg(Color::Red),
                " git am error (x: abort) ",
            ),
            LocalMode::Aborting => (
                format!("Aborting{}", SPINNER[self.spinner]),
                Style::default().fg(Color::Yellow),
                " ktree ",
            ),
        };

        let popup_width = (area.width * 50 / 100).max(30);
        let inner_width = popup_width.saturating_sub(2).max(1) as usize;
        let line_count: u16 = text
            .lines()
            .map(|line| {
                if line.is_empty() {
                    1
                } else {
                    line.len().div_ceil(inner_width) as u16
                }
            })
            .sum::<u16>()
            .max(1);
        let popup_height = (line_count + 2).min(area.height.saturating_sub(2));
        let popup_area = Rect::new(
            area.width.saturating_sub(popup_width + 1),
            1,
            popup_width,
            popup_height,
        );

        let border_type = if self.local_mode == LocalMode::Failed {
            BorderType::Double
        } else {
            BorderType::Rounded
        };

        frame.render_widget(ratatui::widgets::Clear, popup_area);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(style)
                        .border_type(border_type),
                )
                .style(style),
            popup_area,
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn create_temp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("patch-hub-test-{pid}-{id}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn init_repo(path: &Path, branch: &str) {
        StdCommand::new("git")
            .args(["init", "-b", branch])
            .current_dir(path)
            .output()
            .unwrap();
        StdCommand::new("git")
            .args([
                "-c",
                "user.email=test@test.com",
                "-c",
                "user.name=Test",
                "commit",
                "--allow-empty",
                "-m",
                "init",
            ])
            .current_dir(path)
            .output()
            .unwrap();
    }

    #[tokio::test]
    async fn test_apply_single_patch_success() {
        let dir = create_temp_dir();
        init_repo(&dir, "main");

        let patch_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/single_patch.mbox");

        let output = tokio::process::Command::new("git")
            .args(["am", patch_path.to_str().unwrap()])
            .current_dir(&dir)
            .output()
            .await
            .unwrap();

        assert!(
            output.status.success(),
            "git am failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let log = tokio::process::Command::new("git")
            .args(["log", "-1", "--format=%s"])
            .current_dir(&dir)
            .output()
            .await
            .unwrap();
        let subject = String::from_utf8_lossy(&log.stdout).trim().to_string();
        assert_eq!(subject, "Add hello.txt");
        assert!(&dir.join("hello.txt").exists());
    }

    #[tokio::test]
    async fn test_apply_conflict_and_abort() {
        let dir = create_temp_dir();
        init_repo(&dir, "main");

        std::fs::write(dir.join("hello.txt"), "conflict\n").unwrap();
        StdCommand::new("git")
            .args([
                "-c",
                "user.email=t@t.com",
                "-c",
                "user.name=T",
                "commit",
                "-Am",
                "conflict",
            ])
            .current_dir(&dir)
            .output()
            .unwrap();

        let patch_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/single_patch.mbox");

        let output = tokio::process::Command::new("git")
            .args(["am", patch_path.to_str().unwrap()])
            .current_dir(&dir)
            .output()
            .await
            .unwrap();
        assert!(!output.status.success());
        assert!(&dir.join(".git/rebase-apply").exists());

        let abort = tokio::process::Command::new("git")
            .args(["am", "--abort"])
            .current_dir(&dir)
            .output()
            .await
            .unwrap();
        assert!(abort.status.success());
        assert!(!&dir.join(".git/rebase-apply").exists());
    }
}
