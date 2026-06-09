use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, Lines,
    actions::motion::{MoveToFirstRow, MoveToLastRow},
    actions::search::StartSearch,
    actions::{
        Action, CopyLine, CopySelection, FindNext, FindPrevious, MoveBackward, MoveDown,
        MoveForward, MoveHalfPageDown, MoveHalfPageUp, MoveToEndOfLine, MoveToFirst,
        MoveToMatchinBracket, MoveToStartOfLine, MoveUp, MoveWordBackward, MoveWordForward,
        MoveWordForwardToEndOfWord, RemoveCharFromSearch, SelectInnerBetween, SelectInnerWord,
        SelectLine, SwitchMode, TriggerSearch,
    },
    events::{KeyEvent as EditorKeyEvent, KeyEventHandler, KeyEventRegister},
};
use ratatui::layout::{Position, Rect};

use super::{App, panels::MainDisplayTab};
use crate::proxy_handler::CapturedData;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyViewerKey {
    sequence: u64,
    tab: MainDisplayTab,
}

impl BodyViewerKey {
    pub fn new(sequence: u64, tab: MainDisplayTab) -> Self {
        Self { sequence, tab }
    }
}

#[derive(Clone, Debug, Default)]
enum JumpState {
    #[default]
    Inactive,
    Query {
        query: String,
    },
    AwaitRender {
        query: String,
    },
    AwaitLabel {
        query: String,
    },
}

#[derive(Clone)]
pub struct BodyViewer {
    active: bool,
    content_key: Option<BodyViewerKey>,
    editor: Option<EditorState>,
    event_handler: EditorEventHandler,
    jump_state: JumpState,
    jump_targets: Vec<(char, Position)>,
    jump_targets_area: Option<Rect>,
}

impl BodyViewer {
    pub fn new() -> Self {
        Self {
            active: false,
            content_key: None,
            editor: None,
            event_handler: read_only_event_handler(),
            jump_state: JumpState::Inactive,
            jump_targets: Vec::new(),
            jump_targets_area: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn enter(&mut self) {
        self.active = true;
        self.cancel_jump();
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.content_key = None;
        self.editor = None;
        self.event_handler = read_only_event_handler();
        self.cancel_jump();
    }

    pub fn reset(&mut self) {
        self.exit();
    }

    fn load_content(&mut self, key: BodyViewerKey, text: String) {
        if self.content_key == Some(key) && self.editor.is_some() {
            return;
        }

        self.content_key = Some(key);
        self.editor = Some(EditorState::new(Lines::from(text.as_str())));
        self.event_handler = read_only_event_handler();
        self.cancel_jump();
    }

    pub fn has_content_for(&self, key: BodyViewerKey) -> bool {
        self.content_key == Some(key) && self.editor.is_some()
    }

    pub fn editor_mut(&mut self) -> Option<&mut EditorState> {
        self.editor.as_mut()
    }

    pub fn jump_overlay_query(&self, area: Rect) -> Option<&str> {
        match &self.jump_state {
            JumpState::AwaitRender { query } => Some(query),
            JumpState::AwaitLabel { query } if self.jump_targets_area != Some(area) => Some(query),
            JumpState::Inactive | JumpState::Query { .. } => None,
            JumpState::AwaitLabel { .. } => None,
        }
    }

    pub fn rendered_jump_targets(&self, area: Rect) -> &[(char, Position)] {
        if matches!(self.jump_state, JumpState::AwaitLabel { .. }) && self.jump_targets_area == Some(area)
        {
            &self.jump_targets
        } else {
            &[]
        }
    }

    pub fn replace_visible_jump_targets(
        &mut self,
        query: &str,
        area: Rect,
        targets: Vec<(char, Position)>,
        has_matches: bool,
    ) {
        if !matches!(
            &self.jump_state,
            JumpState::AwaitRender { query: active_query }
                | JumpState::AwaitLabel {
                    query: active_query
                } if active_query == query
        ) {
            self.jump_targets.clear();
            self.jump_targets_area = None;
            return;
        }

        if !has_matches {
            self.cancel_jump();
            return;
        }

        self.jump_targets = targets;
        self.jump_targets_area = Some(area);
        self.jump_state = JumpState::AwaitLabel {
            query: query.to_string(),
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if !self.active {
            return false;
        }

        if key.code == KeyCode::Esc && is_plain_key(key) {
            self.exit();
            return true;
        }

        if self.handle_jump_key(key) {
            return true;
        }

        let Some(editor_key) = editor_key_event(key) else {
            return true;
        };
        let Some(editor) = self.editor.as_mut() else {
            return true;
        };

        self.event_handler.on_key_event(editor_key, editor);
        true
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        if !self.active {
            return false;
        }

        self.cancel_jump();

        let Some(editor) = self.editor.as_mut() else {
            return true;
        };

        self.event_handler.on_mouse_event(mouse, editor);
        true
    }

    pub fn scroll_down(&mut self) -> bool {
        self.handle_editor_key(EditorKeyEvent::Ctrl('d'))
    }

    pub fn scroll_up(&mut self) -> bool {
        self.handle_editor_key(EditorKeyEvent::Ctrl('u'))
    }

    fn handle_editor_key(&mut self, key: EditorKeyEvent) -> bool {
        if !self.active {
            return false;
        }

        self.cancel_jump();

        let Some(editor) = self.editor.as_mut() else {
            return true;
        };

        self.event_handler.on_key_event(key, editor);
        true
    }

    fn handle_jump_key(&mut self, key: KeyEvent) -> bool {
        let Some(ch) = plain_char(key) else {
            return false;
        };

        match self.jump_state.clone() {
            JumpState::Inactive => {
                if ch == 's'
                    && self
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.mode == EditorMode::Normal)
                {
                    self.jump_state = JumpState::Query {
                        query: String::new(),
                    };
                    self.jump_targets.clear();
                    self.jump_targets_area = None;
                    return true;
                }
                false
            }
            JumpState::Query { mut query } => {
                query.push(ch);
                self.jump_state = JumpState::AwaitRender { query };
                self.jump_targets.clear();
                self.jump_targets_area = None;
                true
            }
            JumpState::AwaitRender { query } => {
                self.refine_jump_query(query, ch);
                true
            }
            JumpState::AwaitLabel { query } => {
                if let Some((_, position)) = self
                    .jump_targets
                    .iter()
                    .find(|(label, _)| *label == ch)
                    .copied()
                {
                    self.jump_to_visible_cell(position);
                    self.cancel_jump();
                } else {
                    self.refine_jump_query(query, ch);
                }
                true
            }
        }
    }

    fn refine_jump_query(&mut self, mut query: String, ch: char) {
        query.push(ch);
        self.jump_state = JumpState::AwaitRender { query };
        self.jump_targets.clear();
        self.jump_targets_area = None;
    }

    fn jump_to_visible_cell(&mut self, position: Position) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };

        // edtui owns wrapped-line coordinate mapping, so screen-cell jumps enter
        // through its mouse adapter instead of duplicating viewport math here.
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: position.x,
            row: position.y,
            modifiers: KeyModifiers::empty(),
        };
        self.event_handler.on_mouse_event(mouse, editor);
    }

    fn cancel_jump(&mut self) {
        self.jump_state = JumpState::Inactive;
        self.jump_targets.clear();
        self.jump_targets_area = None;
    }
}

impl App {
    pub fn enter_current_body_viewer(&mut self) -> bool {
        if !self.ensure_current_body_viewer_content() {
            return false;
        }

        self.detail_panel.body_viewer.enter();
        true
    }

    pub fn ensure_current_body_viewer_content(&mut self) -> bool {
        let Some(key) = self.current_body_viewer_key() else {
            return false;
        };

        if self.detail_panel.body_viewer.has_content_for(key) {
            return true;
        }

        let Some((key, text)) = self.current_body_text() else {
            return false;
        };
        self.detail_panel.body_viewer.load_content(key, text);
        true
    }

    pub fn current_body_viewer_key(&self) -> Option<BodyViewerKey> {
        let req = self.selected_request()?;
        self.detail_panel
            .active_tab
            .is_body()
            .then(|| BodyViewerKey::new(req.sequence, self.detail_panel.active_tab))
    }

    fn current_body_text(&self) -> Option<(BodyViewerKey, String)> {
        let req = self.selected_request()?;
        let tab = self.detail_panel.active_tab;
        let key = BodyViewerKey::new(req.sequence, tab);
        body_text_for_tab(req, tab).map(|text| (key, text))
    }
}

pub(crate) fn body_text_for_tab(req: &CapturedData, tab: MainDisplayTab) -> Option<String> {
    match tab {
        MainDisplayTab::RequestBody => Some(format_request_body(
            req.req_body.as_deref(),
            &req.req_headers,
        )),
        MainDisplayTab::ResponseBody => Some(format_response_body(req)),
        MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => None,
    }
}

pub(crate) fn format_request_body(body: Option<&str>, headers: &[(String, String)]) -> String {
    match body {
        Some("") => "(Empty body)".to_string(),
        Some(body) if is_form_data(headers) => format_form_body(body),
        Some(body) => body.to_string(),
        None => "(No body)".to_string(),
    }
}

pub(crate) fn format_response_body(req: &CapturedData) -> String {
    match req.res_body.as_deref() {
        Some(body) if req.local_path.is_some() => body.to_string(),
        None if req.local_path.is_some() => String::new(),
        Some("") => "(Empty body)".to_string(),
        Some(body) => format_json_body(body),
        None => "(No body)".to_string(),
    }
}

fn is_form_data(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    })
}

fn format_form_body(body: &str) -> String {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            format!(
                "{}: {}",
                decode_url_component(key),
                decode_url_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_json_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string()))
        .unwrap_or_else(|_| body.to_string())
}

fn decode_url_component(input: &str) -> String {
    let input_bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(input_bytes.len());
    let mut cursor = 0;

    while cursor < input_bytes.len() {
        match input_bytes[cursor] {
            b'%' if cursor + 2 < input_bytes.len() => {
                let h1 = input_bytes[cursor + 1];
                let h2 = input_bytes[cursor + 2];
                if let (Some(hi), Some(lo)) = (hex_to_nibble(h1), hex_to_nibble(h2)) {
                    decoded.push(hi * 16 + lo);
                    cursor += 3;
                    continue;
                }
                decoded.push(input_bytes[cursor]);
                cursor += 1;
            }
            b'+' => {
                decoded.push(b' ');
                cursor += 1;
            }
            byte => {
                decoded.push(byte);
                cursor += 1;
            }
        }
    }

    String::from_utf8(decoded).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into())
}

fn hex_to_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn read_only_event_handler() -> EditorEventHandler {
    EditorEventHandler::new(KeyEventHandler::new(read_only_keymap()))
}

fn read_only_keymap() -> HashMap<KeyEventRegister, Action> {
    let mut keymap = HashMap::new();

    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('/')]),
        StartSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::s([EditorKeyEvent::Enter]),
        TriggerSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::s([EditorKeyEvent::Backspace]),
        RemoveCharFromSearch,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('n')]),
        FindNext,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('N')]),
        FindPrevious,
    );

    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('h'), MoveBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Left, MoveBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('l'), MoveForward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Right, MoveForward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('k'), MoveUp(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Up, MoveUp(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('j'), MoveDown(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Down, MoveDown(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('w'), MoveWordForward(1));
    bind_normal_and_visual(
        &mut keymap,
        EditorKeyEvent::Char('e'),
        MoveWordForwardToEndOfWord(1),
    );
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('b'), MoveWordBackward(1));
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('0'), MoveToStartOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Home, MoveToStartOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('_'), MoveToFirst());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('$'), MoveToEndOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::End, MoveToEndOfLine());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Ctrl('d'), MoveHalfPageDown());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Ctrl('u'), MoveHalfPageUp());
    bind_normal_and_visual(&mut keymap, EditorKeyEvent::Char('G'), MoveToLastRow());
    bind_normal_and_visual(
        &mut keymap,
        EditorKeyEvent::Char('%'),
        MoveToMatchinBracket(),
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('g'), EditorKeyEvent::Char('g')]),
        MoveToFirstRow(),
    );
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('g'), EditorKeyEvent::Char('g')]),
        MoveToFirstRow(),
    );

    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('v')]),
        SwitchMode(EditorMode::Visual),
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('V')]),
        SelectLine,
    );
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('i'), EditorKeyEvent::Char('w')]),
        SelectInnerWord,
    );
    for (opening, closing) in [
        ('"', '"'),
        ('\'', '\''),
        ('(', ')'),
        (')', ')'),
        ('{', '}'),
        ('}', '}'),
        ('[', ']'),
        (']', ']'),
    ] {
        bind(
            &mut keymap,
            KeyEventRegister::v([EditorKeyEvent::Char('i'), EditorKeyEvent::Char(opening)]),
            SelectInnerBetween::new(opening, closing),
        );
    }
    bind(
        &mut keymap,
        KeyEventRegister::v([EditorKeyEvent::Char('y')]),
        CopySelection,
    );
    bind(
        &mut keymap,
        KeyEventRegister::n([EditorKeyEvent::Char('y'), EditorKeyEvent::Char('y')]),
        CopyLine,
    );

    keymap
}

fn bind<A>(keymap: &mut HashMap<KeyEventRegister, Action>, key: KeyEventRegister, action: A)
where
    A: Into<Action>,
{
    keymap.insert(key, action.into());
}

fn bind_normal_and_visual<A>(
    keymap: &mut HashMap<KeyEventRegister, Action>,
    key: EditorKeyEvent,
    action: A,
) where
    A: Into<Action> + Clone,
{
    bind(keymap, KeyEventRegister::n([key]), action.clone());
    bind(keymap, KeyEventRegister::v([key]), action);
}

fn editor_key_event(key: KeyEvent) -> Option<EditorKeyEvent> {
    if key.modifiers == KeyModifiers::CONTROL {
        return match key.code {
            KeyCode::Char(ch) => Some(EditorKeyEvent::Ctrl(ch)),
            KeyCode::PageDown => Some(EditorKeyEvent::Ctrl('d')),
            KeyCode::PageUp => Some(EditorKeyEvent::Ctrl('u')),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(ch) if is_plain_key(key) => Some(EditorKeyEvent::Char(ch)),
        KeyCode::Enter if is_plain_key(key) => Some(EditorKeyEvent::Enter),
        KeyCode::Down if is_plain_key(key) => Some(EditorKeyEvent::Down),
        KeyCode::Up if is_plain_key(key) => Some(EditorKeyEvent::Up),
        KeyCode::Right if is_plain_key(key) => Some(EditorKeyEvent::Right),
        KeyCode::Left if is_plain_key(key) => Some(EditorKeyEvent::Left),
        KeyCode::Backspace if is_plain_key(key) => Some(EditorKeyEvent::Backspace),
        KeyCode::Home if is_plain_key(key) => Some(EditorKeyEvent::Home),
        KeyCode::End if is_plain_key(key) => Some(EditorKeyEvent::End),
        KeyCode::PageDown if is_plain_key(key) => Some(EditorKeyEvent::Ctrl('d')),
        KeyCode::PageUp if is_plain_key(key) => Some(EditorKeyEvent::Ctrl('u')),
        _ => None,
    }
}

fn plain_char(key: KeyEvent) -> Option<char> {
    if !is_plain_key(key) {
        return None;
    }

    match key.code {
        KeyCode::Char(ch) => Some(ch),
        _ => None,
    }
}

fn is_plain_key(key: KeyEvent) -> bool {
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}
