use std::{collections::HashMap, fs::read_to_string};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, HighlightSpacing, List, ListState},
};
use serde::{Deserialize, Serialize};
use strum::Display;

use crate::{action::Action, components::Component};

#[derive(Deserialize, Debug, Clone)]
struct Message {
    #[serde(rename = "m")]
    m_id: String,
    #[serde(rename = "dt")]
    date_time: String,
    #[serde(rename = "refs")]
    refs: Option<Vec<String>>,
    #[serde(rename = "s")]
    subject: String,
    #[serde(rename = "f")]
    from: Vec<Vec<Option<String>>>,
}

struct Node {
    m_id: String,
    last_updated: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Display, Serialize, Deserialize)]
pub enum LocalMode {
    Idle,
    Processing,
    Listing,
    Thread,
}

pub struct Patchsets {
    command_tx: Option<tokio::sync::mpsc::UnboundedSender<Action>>,
    local_mode: LocalMode,
    messages: Vec<Message>,
    map_id_message: HashMap<String, Message>,
    map_id_children: HashMap<String, Vec<String>>,
    roots: Vec<Node>,
    list_index: usize,
    thread: Vec<(usize, String)>,
    thread_index: usize,
    domain: String,
    applying: bool,
}

impl Patchsets {
    pub fn new(domain: String) -> Self {
        Self {
            command_tx: None,
            local_mode: LocalMode::Idle,
            messages: Vec::new(),
            map_id_message: HashMap::new(),
            map_id_children: HashMap::new(),
            roots: Vec::new(),
            list_index: 0,
            thread: Vec::new(),
            thread_index: 0,
            domain,
            applying: false,
        }
    }

    fn prepare_list(&mut self, json_path_str: String) -> color_eyre::Result<()> {
        self.messages = Vec::new();
        self.map_id_message = HashMap::new();
        self.map_id_children = HashMap::new();
        self.roots = Vec::new();
        self.list_index = 0;

        let data = read_to_string(&json_path_str)?;

        let raw_values: Vec<serde_json::Value> = serde_json::from_str(&data)?;

        for val in raw_values {
            if let Ok(message) = serde_json::from_value::<Message>(val) {
                self.messages.push(message);
            }
        }

        for message in &self.messages {
            self.map_id_message
                .insert(message.m_id.clone(), message.clone());
            if let Some(refs) = message.refs.clone() {
                let parent_m_id = refs.last().unwrap().to_owned();
                self.map_id_children
                    .entry(parent_m_id)
                    .or_default()
                    .push(message.m_id.clone());
            } else {
                let root = Node {
                    m_id: message.m_id.clone(),
                    last_updated: String::new(),
                };
                self.roots.push(root);
            }
        }

        for children in self.map_id_children.values_mut() {
            children.sort_unstable_by(|a_m_id, b_m_id| {
                let a_datetime = self
                    .map_id_message
                    .get(a_m_id)
                    .map(|message| message.date_time.as_str())
                    .unwrap_or("");
                let b_datetime = self
                    .map_id_message
                    .get(b_m_id)
                    .map(|m| m.date_time.as_str())
                    .unwrap_or("");

                // Compare strings directly since they are ISO 8601
                // This sorts them descending (latest replies first).
                b_datetime.cmp(a_datetime)
            });
        }

        for root in &mut self.roots {
            let mut max_ts = String::new();
            // Use a simple vector as a stack for an iterative DFS
            let mut stack = vec![root.m_id.clone()];

            while let Some(current_m_id) = stack.pop() {
                if let Some(message) = self.map_id_message.get(&current_m_id) {
                    // String comparison works perfectly for ISO 8601
                    if message.date_time > max_ts {
                        max_ts = message.date_time.clone();
                    }
                }

                // If this message has replies, push them to the stack to be processed
                if let Some(children) = self.map_id_children.get(&current_m_id) {
                    stack.extend(children.iter().cloned());
                }
            }

            // Update the root with the highest timestamp found in its thread tree
            root.last_updated = max_ts;
        }

        self.roots
            .sort_unstable_by(|a, b| b.last_updated.cmp(&a.last_updated));

        self.local_mode = LocalMode::Listing;
        Ok(())
    }

    fn open_thread(&mut self) {
        self.thread = Vec::new();
        self.thread_index = 0;

        let root_m_id = self.roots.get(self.list_index).unwrap().m_id.clone();

        let mut stack = vec![(0, root_m_id)];

        while let Some((i, current_m_id)) = stack.pop() {
            if self.map_id_message.contains_key(&current_m_id) {
                self.thread.push((i, current_m_id.clone()));
            }

            // If this message has replies, push them to the stack to be processed
            if let Some(children) = self.map_id_children.get(&current_m_id) {
                stack.extend(children.iter().map(|c| (i + 1, c.clone())));
            }
        }

        self.local_mode = LocalMode::Thread;
    }

    fn download_and_apply(&mut self, message_id: String) {
        self.applying = true;
        let tx = self.command_tx.clone().unwrap();
        let domain = self.domain.clone();
        tx.send(Action::KtreeSetMode(
            crate::components::ktree::LocalMode::Applying,
        ))
        .ok();

        tokio::spawn(async move {
            use std::sync::atomic::{AtomicU64, Ordering};
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let id = COUNTER.fetch_add(1, Ordering::Relaxed);
            let tmp_path = format!(
                "{}/patch-hub-{}-{id}.mbox",
                std::env::temp_dir().display(),
                std::process::id()
            );

            let url = format!("https://{domain}/all/{message_id}/raw");
            let output = tokio::process::Command::new("curl")
                .args(["-sf", "-H", "User-Agent: patch-hub", "-o", &tmp_path, &url])
                .output()
                .await;

            match output {
                Ok(out) if out.status.success() => {
                    let _ = tx.send(Action::KtreeApply(tmp_path));
                }
                Ok(out) => {
                    let _ = tx.send(Action::KtreeResult(crate::action::KtreeStatus::Failed(
                        format!("download failed: HTTP {}", out.status),
                    )));
                    let _ = std::fs::remove_file(&tmp_path);
                }
                Err(e) => {
                    let _ = tx.send(Action::KtreeResult(crate::action::KtreeStatus::Failed(
                        format!("download failed: {e}"),
                    )));
                    let _ = std::fs::remove_file(&tmp_path);
                }
            }
        });
    }
}

impl Component for Patchsets {
    fn register_action_handler(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<Action>,
    ) -> color_eyre::Result<()> {
        self.command_tx = Some(tx);
        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> color_eyre::Result<Option<Action>> {
        let action = match self.local_mode {
            LocalMode::Listing => match key.code {
                KeyCode::Char('j') => Some(Action::PatchsetsAddIndex),
                KeyCode::Char('k') => Some(Action::PatchsetsSubIndex),
                KeyCode::Enter => Some(Action::PatchsetsThread),
                _ => None,
            },
            LocalMode::Thread => match key.code {
                KeyCode::Char('j') => Some(Action::PatchsetsAddIndex),
                KeyCode::Char('k') => Some(Action::PatchsetsSubIndex),
                KeyCode::Esc => Some(Action::PatchsetsSetMode(LocalMode::Listing)),
                KeyCode::Char('a') => {
                    if !self.applying
                        && let Some((_, m_id)) = self.thread.get(self.thread_index)
                    {
                        self.download_and_apply(m_id.clone());
                    }
                    None
                }
                _ => None,
            },
            _ => None,
        };

        Ok(action)
    }

    fn update(&mut self, action: Action) -> color_eyre::Result<Option<Action>> {
        match action {
            Action::PatchsetsSetMode(local_mode) => self.local_mode = local_mode,
            Action::PatchsetsList(json_path) => self.prepare_list(json_path)?,
            Action::PatchsetsAddIndex => match self.local_mode {
                LocalMode::Listing if self.list_index < self.roots.len() - 1 => {
                    self.list_index += 1;
                }
                LocalMode::Thread if self.thread_index < self.thread.len() - 1 => {
                    self.thread_index += 1;
                }
                _ => {}
            },
            Action::PatchsetsSubIndex => match self.local_mode {
                LocalMode::Listing => self.list_index = self.list_index.saturating_sub(1),
                LocalMode::Thread => self.thread_index = self.thread_index.saturating_sub(1),
                _ => {}
            },
            Action::PatchsetsThread => self.open_thread(),
            Action::KtreeResult(_) => self.applying = false,
            _ => {}
        }
        Ok(None)
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect) -> color_eyre::Result<()> {
        if self.local_mode == LocalMode::Listing {
            let rects = Layout::default()
                .constraints(
                    [
                        Constraint::Min(1),
                        Constraint::Percentage(100),
                        Constraint::Min(4),
                    ]
                    .as_ref(),
                )
                .split(area);
            let rects = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(
                    [
                        Constraint::Min(1),
                        Constraint::Percentage(100),
                        Constraint::Min(1),
                    ]
                    .as_ref(),
                )
                .split(rects[1]);

            let list_items: Vec<String> = self
                .roots
                .iter()
                .map(|root| {
                    let message = self.map_id_message.get(&root.m_id).unwrap().clone();
                    let author_name = if let Some(name) = message.from[0][0].as_ref() {
                        name
                    } else {
                        "null"
                    };
                    let author_email = message.from[0][1].as_ref().unwrap();
                    format!("{} | {} <{}>", message.subject, author_name, author_email)
                })
                .collect();
            let list_block = Block::default().borders(Borders::NONE);
            let list = List::new(list_items)
                .block(list_block)
                .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
                .highlight_symbol(">")
                .highlight_spacing(HighlightSpacing::Always);

            let mut list_state = ListState::default();
            list_state.select(Some(self.list_index));

            frame.render_stateful_widget(list, rects[1], &mut list_state);
        } else if self.local_mode == LocalMode::Thread {
            let rects = Layout::default()
                .constraints(
                    [
                        Constraint::Min(1),
                        Constraint::Percentage(100),
                        Constraint::Min(4),
                    ]
                    .as_ref(),
                )
                .split(area);
            let rects = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(
                    [
                        Constraint::Min(1),
                        Constraint::Percentage(100),
                        Constraint::Min(1),
                    ]
                    .as_ref(),
                )
                .split(rects[1]);

            let list_items: Vec<String> = self
                .thread
                .iter()
                .map(|i_and_m_id| {
                    let (i, m_id) = i_and_m_id;
                    let message = self.map_id_message.get(m_id).unwrap().clone();
                    let author_name = if let Some(name) = message.from[0][0].as_ref() {
                        name
                    } else {
                        "null"
                    };
                    let author_email = message.from[0][1].as_ref().unwrap();
                    let line = format!("{} | {} <{}>", message.subject, author_name, author_email);

                    if *i != 0 {
                        format!("{}\u{21B3} {}", "  ".repeat(*i), line)
                    } else {
                        line
                    }
                })
                .collect();
            let list_block = Block::default().borders(Borders::NONE);
            let list = List::new(list_items)
                .block(list_block)
                .highlight_style(Style::default().fg(Color::Black).bg(Color::LightYellow))
                .highlight_symbol(">")
                .highlight_spacing(HighlightSpacing::Always);

            let mut list_state = ListState::default();
            list_state.select(Some(self.thread_index));

            frame.render_stateful_widget(list, rects[1], &mut list_state);
        }
        Ok(())
    }
}
