use std::{fs::File, process::Stdio};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use serde::{Deserialize, Serialize};
use strum::Display;
use tokio::{process::Command, sync::mpsc::UnboundedSender};
use tracing::{error, info};

use crate::{
    action::Action,
    components::{Component, SPINNER, patchsets},
    config::Config,
};

const MAX_NOTIFICATION_TICKS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum LocalMode {
    Idle,
    Processing,
    ExitingProcessing,
}

pub struct Lei {
    command_tx: Option<UnboundedSender<Action>>,
    config: Config,
    local_mode: LocalMode,
    spinner: usize,
    notification_ticks: usize,
    domain: String,
    list: String,
    query: String,
}

impl Lei {
    pub fn new(domain: String, list: String, query: String) -> Self {
        Self {
            command_tx: None,
            config: Config::default(),
            local_mode: LocalMode::Idle,
            spinner: 0,
            notification_ticks: 0,
            domain,
            list,
            query,
        }
    }

    fn fetch_patchsets(&self) {
        let tx = self.command_tx.clone().unwrap();
        let mut data_dir = self.config.config.data_dir.clone();
        let domain = self.domain.clone();
        let list = self.list.clone();
        let query = self.query.clone();

        tokio::spawn(async move {
            tx.send(Action::LeiSetMode(LocalMode::Processing)).unwrap();

            data_dir.push(list.as_str());
            let inbox_dir_str = data_dir.to_str().unwrap();
            let mut data_dir = data_dir.clone();
            data_dir.pop();
            data_dir.push(format!("{list}.json"));
            let json_path_str = data_dir.to_str().unwrap().to_string();

            info!("fetching patchsets from {inbox_dir_str}");
            let _ = tokio::fs::create_dir_all(data_dir.parent().unwrap_or(&data_dir)).await;
            tx.send(Action::PatchsetsSetMode(patchsets::LocalMode::Processing))
                .unwrap();
            if let Ok(output) = File::create(&json_path_str) {
                if let Ok(exit_status) = Command::new("lei")
                    .arg("q")
                    .arg(format!("--only=https://{domain}/{list}/"))
                    .arg("--no-local")
                    .arg("--threads")
                    .arg("--dedupe=mid")
                    .arg(&query)
                    .stdout(output)
                    .stderr(Stdio::null())
                    .status()
                    .await
                {
                    if !exit_status.success() {
                        error!("lei command exit status was unsuccessful {exit_status:?}");
                    }

                    tx.send(Action::PatchsetsList(json_path_str)).unwrap();
                } else {
                    error!("failed to execute command");
                }
            } else {
                error!("failed to create {json_path_str}");
            }

            tx.send(Action::LeiSetMode(LocalMode::ExitingProcessing))
                .unwrap();
        });
    }
}

impl Component for Lei {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn register_config_handler(&mut self, config: Config) -> color_eyre::Result<()> {
        self.config = config;
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        let action = match self.local_mode {
            LocalMode::Idle => match key.code {
                KeyCode::Char('f') => Some(Action::LeiFetchPatchsets),
                _ => None,
            },
            _ => None,
        };

        Ok(action)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::Tick => {
                if self.local_mode == LocalMode::ExitingProcessing {
                    self.notification_ticks += 1;
                    if self.notification_ticks >= MAX_NOTIFICATION_TICKS {
                        self.notification_ticks = 0;
                        return Ok(Some(Action::LeiSetMode(LocalMode::Idle)));
                    }
                } else if self.local_mode != LocalMode::Idle {
                    self.spinner += 1;
                    if self.spinner >= SPINNER.len() {
                        self.spinner = 0;
                    }
                }
            }
            Action::LeiSetMode(local_mode) => match local_mode {
                LocalMode::Idle => self.local_mode = local_mode,
                LocalMode::Processing => {
                    self.spinner = 0;
                    self.local_mode = LocalMode::Processing;
                }
                LocalMode::ExitingProcessing => {
                    self.notification_ticks = 0;
                    self.local_mode = LocalMode::ExitingProcessing;
                }
            },
            Action::LeiFetchPatchsets => self.fetch_patchsets(),
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        let rects = Layout::default()
            .constraints([Constraint::Percentage(100), Constraint::Min(3)].as_ref())
            .split(area);
        let rects = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(
                [
                    Constraint::Percentage(30),
                    Constraint::Percentage(40),
                    Constraint::Percentage(30),
                ]
                .as_ref(),
            )
            .split(rects[1]);

        let text = format!("{}/{}", self.domain, self.list);
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .title(" target ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan))
                        .border_type(BorderType::Rounded),
                )
                .style(Style::default().fg(Color::Cyan))
                .alignment(Alignment::Center),
            rects[0],
        );

        let text = self.query.to_string();
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .title(" query ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan))
                        .border_type(BorderType::Rounded),
                )
                .style(Style::default().fg(Color::Cyan))
                .alignment(Alignment::Center),
            rects[1],
        );

        let text = match self.local_mode {
            LocalMode::Idle => "Idle".to_string(),
            LocalMode::Processing => format!("Processing{}", SPINNER[self.spinner]),
            LocalMode::ExitingProcessing => "Finished processing!".to_string(),
        };
        let style = match self.local_mode {
            LocalMode::Idle | LocalMode::ExitingProcessing => Style::default().fg(Color::Cyan),
            LocalMode::Processing => Style::default().fg(Color::Yellow),
        };
        frame.render_widget(
            Paragraph::new(text)
                .block(
                    Block::default()
                        .title(" status ")
                        .title_alignment(Alignment::Center)
                        .borders(Borders::ALL)
                        .border_style(style)
                        .border_type(BorderType::Rounded),
                )
                .style(style)
                .alignment(Alignment::Center),
            rects[2],
        );
        Ok(())
    }
}
