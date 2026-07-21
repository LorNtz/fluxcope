use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use edtui::{
    EditorEventHandler, EditorMode, EditorState, Index2, Lines,
    actions::motion::{MoveToFirstRow, MoveToLastRow},
    actions::search::StartSearch,
    actions::{
        Action, CopyLine, CopySelection, Execute, FindNext, FindPrevious, MoveBackward, MoveDown,
        MoveForward, MoveHalfPageDown, MoveHalfPageUp, MoveToEndOfLine, MoveToFirst,
        MoveToMatchinBracket, MoveToStartOfLine, MoveUp, MoveWordBackward, MoveWordForward,
        MoveWordForwardToEndOfWord, RemoveCharFromSearch, SelectInnerBetween, SelectInnerWord,
        SelectLine, SwitchMode, TriggerSearch,
    },
    events::{KeyEvent as EditorKeyEvent, KeyEventHandler, KeyEventRegister},
};
use ratatui::layout::{Position, Rect};

use super::{App, panels::MainDisplayTab};
use crate::capture::{CaptureSequence, CapturedExchange};

pub(crate) const BODY_TEXT_TAB: &str = "    ";
pub(crate) const BODY_TEXT_TAB_WIDTH: usize = BODY_TEXT_TAB.len();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BodyViewerKey {
    sequence: CaptureSequence,
    tab: MainDisplayTab,
}

impl BodyViewerKey {
    pub fn new(sequence: CaptureSequence, tab: MainDisplayTab) -> Self {
        Self { sequence, tab }
    }

    pub fn tab(self) -> MainDisplayTab {
        self.tab
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum YankState {
    #[default]
    Inactive,
    Pending,
    PendingG,
    PendingInner,
}

#[derive(Clone, Copy, Debug)]
enum YankCommand {
    CopyLine,
    Motion {
        movement: YankMovement,
        selection: YankSelection,
    },
    TextObject(YankTextObject),
    Continue(YankState),
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum YankMovement {
    Backward,
    Down,
    EndOfLine,
    FirstRow,
    Forward,
    LastRow,
    MatchingBracket,
    StartOfLine,
    Up,
    WordBackward,
    WordForward,
    WordForwardEnd,
}

#[derive(Clone, Copy, Debug)]
enum YankSelection {
    CharInclusive,
    CharExclusiveBackward,
    CharExclusiveForward { include_line_end: bool },
    Linewise,
}

#[derive(Clone, Copy, Debug)]
enum YankTextObject {
    Word,
    Between { opening: char, closing: char },
}

#[derive(Clone)]
pub struct BodyViewer {
    active: bool,
    content_key: Option<BodyViewerKey>,
    editor: Option<EditorState>,
    event_handler: EditorEventHandler,
    jump_state: JumpState,
    yank_state: YankState,
    cached_jump_targets: Vec<(char, Position)>,
    area_where_jump_targets_computed: Option<Rect>,
}

impl BodyViewer {
    pub fn new() -> Self {
        Self {
            active: false,
            content_key: None,
            editor: None,
            event_handler: read_only_event_handler(),
            jump_state: JumpState::Inactive,
            yank_state: YankState::Inactive,
            cached_jump_targets: Vec::new(),
            area_where_jump_targets_computed: None,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn enter(&mut self) {
        self.active = true;
        self.cancel_jump();
        self.cancel_yank();
    }

    pub fn exit(&mut self) {
        self.active = false;
        self.content_key = None;
        self.editor = None;
        self.event_handler = read_only_event_handler();
        self.cancel_jump();
        self.cancel_yank();
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
        self.cancel_yank();
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
            JumpState::AwaitLabel { query }
                if self.area_where_jump_targets_computed != Some(area) =>
            {
                Some(query)
            }
            JumpState::Inactive | JumpState::Query { .. } => None,
            JumpState::AwaitLabel { .. } => None,
        }
    }

    pub fn rendered_jump_targets(&self, area: Rect) -> &[(char, Position)] {
        if matches!(self.jump_state, JumpState::AwaitLabel { .. })
            && self.area_where_jump_targets_computed == Some(area)
        {
            &self.cached_jump_targets
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
            self.cached_jump_targets.clear();
            self.area_where_jump_targets_computed = None;
            return;
        }

        if !has_matches {
            self.cancel_jump();
            return;
        }

        self.cached_jump_targets = targets;
        self.area_where_jump_targets_computed = Some(area);
        self.jump_state = JumpState::AwaitLabel {
            query: query.to_string(),
        };
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        if !self.active {
            return false;
        }

        if key.code == KeyCode::Esc && is_plain_key(key) && self.yank_state != YankState::Inactive {
            self.cancel_yank();
            return true;
        }

        if key.code == KeyCode::Esc && is_plain_key(key) {
            self.exit();
            return true;
        }

        if self.handle_jump_key(key) {
            return true;
        }

        if self.handle_yank_key(key) {
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
        self.cancel_yank();

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
        self.cancel_yank();

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
                    && self.yank_state == YankState::Inactive
                    && self
                        .editor
                        .as_ref()
                        .is_some_and(|editor| editor.mode == EditorMode::Normal)
                {
                    self.jump_state = JumpState::Query {
                        query: String::new(),
                    };
                    self.cached_jump_targets.clear();
                    self.area_where_jump_targets_computed = None;
                    return true;
                }
                false
            }
            JumpState::Query { mut query } => {
                query.push(ch);
                self.jump_state = JumpState::AwaitRender { query };
                self.cached_jump_targets.clear();
                self.area_where_jump_targets_computed = None;
                true
            }
            JumpState::AwaitRender { query } => {
                self.refine_jump_query(query, ch);
                true
            }
            JumpState::AwaitLabel { query } => {
                if let Some((_, position)) = self
                    .cached_jump_targets
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
        self.cached_jump_targets.clear();
        self.area_where_jump_targets_computed = None;
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
        self.cached_jump_targets.clear();
        self.area_where_jump_targets_computed = None;
    }

    fn handle_yank_key(&mut self, key: KeyEvent) -> bool {
        if self.yank_state == YankState::Inactive {
            if plain_char(key) == Some('y')
                && self
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.mode == EditorMode::Normal)
            {
                self.yank_state = YankState::Pending;
                return true;
            }

            return false;
        }

        let command = match self.yank_state {
            YankState::Inactive => return false,
            YankState::Pending => pending_yank_command(key),
            YankState::PendingG => pending_g_yank_command(key),
            YankState::PendingInner => pending_inner_yank_command(key),
        };

        match command {
            YankCommand::Continue(next_state) => {
                self.yank_state = next_state;
            }
            YankCommand::Cancel => {
                self.cancel_yank();
            }
            YankCommand::CopyLine => {
                if let Some(editor) = self.editor.as_mut() {
                    CopyLine.execute(editor);
                }
                self.cancel_yank();
            }
            YankCommand::Motion {
                movement,
                selection,
            } => {
                if let Some(editor) = self.editor.as_mut() {
                    copy_yank_motion(editor, movement, selection);
                }
                self.cancel_yank();
            }
            YankCommand::TextObject(text_object) => {
                if let Some(editor) = self.editor.as_mut() {
                    copy_yank_text_object(editor, text_object);
                }
                self.cancel_yank();
            }
        }

        true
    }

    fn cancel_yank(&mut self) {
        self.yank_state = YankState::Inactive;
    }
}

fn pending_yank_command(key: KeyEvent) -> YankCommand {
    match editor_key_event(key) {
        Some(EditorKeyEvent::Char('y')) | Some(EditorKeyEvent::Char('_')) => YankCommand::CopyLine,
        Some(EditorKeyEvent::Char('h')) | Some(EditorKeyEvent::Left) => {
            yank_motion(YankMovement::Backward, YankSelection::CharExclusiveBackward)
        }
        Some(EditorKeyEvent::Char('l')) | Some(EditorKeyEvent::Right) => yank_motion(
            YankMovement::Forward,
            YankSelection::CharExclusiveForward {
                include_line_end: false,
            },
        ),
        Some(EditorKeyEvent::Char('j')) | Some(EditorKeyEvent::Down) => {
            yank_motion(YankMovement::Down, YankSelection::Linewise)
        }
        Some(EditorKeyEvent::Char('k')) | Some(EditorKeyEvent::Up) => {
            yank_motion(YankMovement::Up, YankSelection::Linewise)
        }
        Some(EditorKeyEvent::Char('w')) => yank_motion(
            YankMovement::WordForward,
            YankSelection::CharExclusiveForward {
                include_line_end: true,
            },
        ),
        Some(EditorKeyEvent::Char('e')) => {
            yank_motion(YankMovement::WordForwardEnd, YankSelection::CharInclusive)
        }
        Some(EditorKeyEvent::Char('b')) => yank_motion(
            YankMovement::WordBackward,
            YankSelection::CharExclusiveBackward,
        ),
        Some(EditorKeyEvent::Char('0')) | Some(EditorKeyEvent::Home) => yank_motion(
            YankMovement::StartOfLine,
            YankSelection::CharExclusiveBackward,
        ),
        Some(EditorKeyEvent::Char('$')) | Some(EditorKeyEvent::End) => {
            yank_motion(YankMovement::EndOfLine, YankSelection::CharInclusive)
        }
        Some(EditorKeyEvent::Char('G')) => {
            yank_motion(YankMovement::LastRow, YankSelection::Linewise)
        }
        Some(EditorKeyEvent::Char('%')) => {
            yank_motion(YankMovement::MatchingBracket, YankSelection::CharInclusive)
        }
        Some(EditorKeyEvent::Char('g')) => YankCommand::Continue(YankState::PendingG),
        Some(EditorKeyEvent::Char('i')) => YankCommand::Continue(YankState::PendingInner),
        _ => YankCommand::Cancel,
    }
}

fn pending_g_yank_command(key: KeyEvent) -> YankCommand {
    match editor_key_event(key) {
        Some(EditorKeyEvent::Char('g')) => {
            yank_motion(YankMovement::FirstRow, YankSelection::Linewise)
        }
        _ => YankCommand::Cancel,
    }
}

fn pending_inner_yank_command(key: KeyEvent) -> YankCommand {
    match editor_key_event(key) {
        Some(EditorKeyEvent::Char('w')) => YankCommand::TextObject(YankTextObject::Word),
        Some(EditorKeyEvent::Char(ch)) => inner_between_delimiters(ch)
            .map(|(opening, closing)| {
                YankCommand::TextObject(YankTextObject::Between { opening, closing })
            })
            .unwrap_or(YankCommand::Cancel),
        _ => YankCommand::Cancel,
    }
}

fn yank_motion(movement: YankMovement, selection: YankSelection) -> YankCommand {
    YankCommand::Motion {
        movement,
        selection,
    }
}

fn inner_between_delimiters(ch: char) -> Option<(char, char)> {
    match ch {
        '"' => Some(('"', '"')),
        '\'' => Some(('\'', '\'')),
        '(' | ')' => Some(('(', ')')),
        '{' | '}' => Some(('{', '}')),
        '[' | ']' => Some(('[', ']')),
        _ => None,
    }
}

fn copy_yank_motion(editor: &mut EditorState, movement: YankMovement, selection: YankSelection) {
    let saved_cursor = editor.cursor;
    let saved_mode = editor.mode;
    let saved_selection = editor.selection.clone();

    match selection {
        YankSelection::Linewise => SelectLine.execute(editor),
        YankSelection::CharInclusive
        | YankSelection::CharExclusiveBackward
        | YankSelection::CharExclusiveForward { .. } => {
            SwitchMode(EditorMode::Visual).execute(editor)
        }
    }

    movement.execute(editor);
    if adjust_yank_selection(editor, saved_cursor, movement, selection) {
        CopySelection.execute(editor);
    }

    editor.cursor = saved_cursor;
    editor.mode = saved_mode;
    editor.selection = saved_selection;
}

fn copy_yank_text_object(editor: &mut EditorState, text_object: YankTextObject) {
    let saved_cursor = editor.cursor;
    let saved_mode = editor.mode;
    let saved_selection = editor.selection.clone();

    editor.selection = None;
    match text_object {
        YankTextObject::Word => SelectInnerWord.execute(editor),
        YankTextObject::Between { opening, closing } => {
            SelectInnerBetween::new(opening, closing).execute(editor);
        }
    }

    if editor.selection.is_some() {
        CopySelection.execute(editor);
    }

    editor.cursor = saved_cursor;
    editor.mode = saved_mode;
    editor.selection = saved_selection;
}

fn adjust_yank_selection(
    editor: &mut EditorState,
    origin: Index2,
    movement: YankMovement,
    selection: YankSelection,
) -> bool {
    let destination = editor.cursor;

    match selection {
        YankSelection::Linewise => true,
        YankSelection::CharInclusive => {
            if movement == YankMovement::MatchingBracket && destination == origin {
                return false;
            }

            if is_virtual_line_end(editor, destination) {
                let Some(end) = previous_position(editor, destination) else {
                    return false;
                };

                if let Some(selection) = editor.selection.as_mut() {
                    selection.end = end;
                }
            }

            true
        }
        YankSelection::CharExclusiveForward { include_line_end } => {
            if destination == origin {
                return false;
            }

            if !include_line_end || !is_line_end(editor, destination) {
                let Some(end) = previous_position(editor, destination) else {
                    return false;
                };

                if let Some(selection) = editor.selection.as_mut() {
                    selection.end = end;
                }
            }

            true
        }
        YankSelection::CharExclusiveBackward => {
            if destination == origin {
                return false;
            }

            let Some(start) = previous_position(editor, origin) else {
                return false;
            };

            if let Some(selection) = editor.selection.as_mut() {
                selection.start = start;
            }

            true
        }
    }
}

fn is_line_end(editor: &EditorState, position: Index2) -> bool {
    editor
        .lines
        .len_col(position.row)
        .is_some_and(|len| position.col >= len.saturating_sub(1))
}

fn is_virtual_line_end(editor: &EditorState, position: Index2) -> bool {
    editor
        .lines
        .len_col(position.row)
        .is_some_and(|len| position.col >= len)
}

fn previous_position(editor: &EditorState, position: Index2) -> Option<Index2> {
    if position.col > 0 {
        return Some(Index2::new(position.row, position.col - 1));
    }

    if position.row == 0 {
        return None;
    }

    editor
        .lines
        .len_col(position.row - 1)
        .map(|len| Index2::new(position.row - 1, len.saturating_sub(1)))
}

impl YankMovement {
    fn execute(self, editor: &mut EditorState) {
        match self {
            YankMovement::Backward => MoveBackward(1).execute(editor),
            YankMovement::Down => MoveDown(1).execute(editor),
            YankMovement::EndOfLine => MoveToEndOfLine().execute(editor),
            YankMovement::FirstRow => MoveToFirstRow().execute(editor),
            YankMovement::Forward => MoveForward(1).execute(editor),
            YankMovement::LastRow => MoveToLastRow().execute(editor),
            YankMovement::MatchingBracket => MoveToMatchinBracket().execute(editor),
            YankMovement::StartOfLine => MoveToStartOfLine().execute(editor),
            YankMovement::Up => MoveUp(1).execute(editor),
            YankMovement::WordBackward => MoveWordBackward(1).execute(editor),
            YankMovement::WordForward => MoveWordForward(1).execute(editor),
            YankMovement::WordForwardEnd => MoveWordForwardToEndOfWord(1).execute(editor),
        }
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
        let sequence = self.selected_request_sequence()?;
        self.detail_panel
            .active_tab
            .is_body()
            .then(|| BodyViewerKey::new(sequence, self.detail_panel.active_tab))
    }

    pub(crate) fn ensure_body_text_cached(&mut self, key: BodyViewerKey) -> bool {
        if self.cached_body_text(key).is_some() {
            return true;
        }

        let text = {
            let Some(req) = self.selected_request() else {
                return false;
            };

            if BodyViewerKey::new(req.sequence, key.tab()) != key {
                return false;
            }

            body_text_for_tab(req, key.tab())
        };
        let Some(text) = text else {
            return false;
        };

        self.detail_panel.cache_body_text(key, text);
        true
    }

    pub(crate) fn cached_body_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.detail_panel.cached_body_text(key)
    }

    pub(crate) fn cached_body_render_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.detail_panel.cached_body_render_text(key)
    }

    fn current_body_text(&mut self) -> Option<(BodyViewerKey, String)> {
        let key = self.current_body_viewer_key()?;
        if !self.ensure_body_text_cached(key) {
            return None;
        }

        self.detail_panel
            .take_cached_body_text(key)
            .map(|text| (key, text))
    }
}

pub(crate) fn body_text_for_tab(req: &CapturedExchange, tab: MainDisplayTab) -> Option<String> {
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
        Some(body) => format_json_body(body),
        None => "(No body)".to_string(),
    }
}

pub(crate) fn format_response_body(req: &CapturedExchange) -> String {
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
