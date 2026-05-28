use crate::{
    proxy_handler::CapturedData,
    settings::{RequestListSettings, UiSettings},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;
use std::collections::HashSet;
use tui_tree_widget::TreeState;
use url::Url;

const ORIGIN_IDENTIFIER_PREFIX: &str = "origin:";
const SEGMENT_IDENTIFIER_PREFIX: &str = "segment:";
const REQUEST_IDENTIFIER_PREFIX: &str = "request:";
const FOCUSABLE_PANELS_WITH_LOG: [PanelFocus; 3] =
    [PanelFocus::RequestList, PanelFocus::Detail, PanelFocus::Log];
const FOCUSABLE_PANELS_WITHOUT_LOG: [PanelFocus; 2] = [PanelFocus::RequestList, PanelFocus::Detail];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelFocus {
    RequestList,
    Detail,
    Log,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupFocus {
    Certificate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FocusState {
    panel: PanelFocus,
    popup: Option<PopupFocus>,
}

impl FocusState {
    fn new() -> Self {
        Self {
            panel: PanelFocus::RequestList,
            popup: None,
        }
    }

    fn panel(self) -> PanelFocus {
        self.panel
    }

    fn popup(self) -> Option<PopupFocus> {
        self.popup
    }

    fn focus_panel(&mut self, panel: PanelFocus) {
        self.panel = panel;
    }

    fn open_popup(&mut self, popup: PopupFocus) {
        self.popup = Some(popup);
    }

    fn close_popup(&mut self) {
        self.popup = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusDirection {
    Left,
    Down,
    Up,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MainDisplayTab {
    RequestHeader,
    RequestBody,
    ResponseHeader,
    ResponseBody,
}

impl MainDisplayTab {
    pub fn title(self) -> &'static str {
        match self {
            MainDisplayTab::RequestHeader => "Request Header",
            MainDisplayTab::RequestBody => "Request Body",
            MainDisplayTab::ResponseHeader => "Response Header",
            MainDisplayTab::ResponseBody => "Response Body",
        }
    }

    pub fn index(self) -> usize {
        match self {
            MainDisplayTab::RequestHeader => 0,
            MainDisplayTab::RequestBody => 1,
            MainDisplayTab::ResponseHeader => 2,
            MainDisplayTab::ResponseBody => 3,
        }
    }
}

#[derive(Debug)]
pub enum AppEvent {
    NetworkRequest(CapturedData),
    LogMessage(String),
    CertificateDownloadReady(String),
}

pub struct ScrollState {
    pub offset: u16,
    pub max_offset: u16,
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            max_offset: 0,
        }
    }

    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1).min(self.max_offset);
    }

    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

pub struct RequestListPanel {
    pub state: TreeState<String>,
    auto_expand: bool,
}

impl RequestListPanel {
    pub fn new(setting: RequestListSettings) -> Self {
        Self {
            state: TreeState::default(),
            auto_expand: setting.auto_expand,
        }
    }

    fn open_entry(&mut self, entry: &RequestTreeEntry) {
        for branch in entry.branch_paths() {
            self.state.open(branch);
        }
    }

    fn select_entry(&mut self, entry: &RequestTreeEntry) {
        self.state.select(entry.request_path());
    }

    fn select_origin(&mut self, entry: &RequestTreeEntry) {
        self.state.select(vec![origin_identifier(&entry.origin)]);
    }

    pub fn click_at(&mut self, position: Position) -> bool {
        self.state.click_at(position)
    }

    pub fn scroll_down(&mut self) -> bool {
        self.state.scroll_down(1)
    }

    pub fn scroll_up(&mut self) -> bool {
        self.state.scroll_up(1)
    }
}

pub struct DetailPanel {
    pub active_tab: MainDisplayTab,
    pub scroll: ScrollState,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
        }
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            MainDisplayTab::RequestHeader => MainDisplayTab::RequestBody,
            MainDisplayTab::RequestBody => MainDisplayTab::ResponseHeader,
            MainDisplayTab::ResponseHeader => MainDisplayTab::ResponseBody,
            MainDisplayTab::ResponseBody => MainDisplayTab::RequestHeader,
        };
        self.scroll.reset();
    }

    pub fn previous_tab(&mut self) {
        self.active_tab = match self.active_tab {
            MainDisplayTab::RequestHeader => MainDisplayTab::ResponseBody,
            MainDisplayTab::RequestBody => MainDisplayTab::RequestHeader,
            MainDisplayTab::ResponseHeader => MainDisplayTab::RequestBody,
            MainDisplayTab::ResponseBody => MainDisplayTab::ResponseHeader,
        };
        self.scroll.reset();
    }
}

pub struct LogPanel {
    pub logs: Vec<String>,
    pub scroll: ScrollState,
    pub visible: bool,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            logs: vec![],
            scroll: ScrollState::new(),
            visible: true,
        }
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(msg);
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

pub struct CertificatePopup {
    pub visible: bool,
    pub download_url: Option<String>,
}

impl CertificatePopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            download_url: None,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn set_download_url(&mut self, download_url: String) {
        self.download_url = Some(download_url);
    }
}

pub struct App {
    pub requests: Vec<CapturedData>,
    pub recording: bool,
    focus: FocusState,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
    pub certificate_popup: CertificatePopup,
}

impl App {
    pub fn new(ui_settings: UiSettings) -> Self {
        Self {
            requests: vec![],
            recording: true,
            focus: FocusState::new(),
            request_list: RequestListPanel::new(ui_settings.request_list),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::new(),
            certificate_popup: CertificatePopup::new(),
        }
    }

    pub fn focused_panel(&self) -> PanelFocus {
        self.focus.panel()
    }

    pub fn focused_popup(&self) -> Option<PopupFocus> {
        self.focus.popup()
    }

    pub fn is_panel_focused(&self, panel: PanelFocus) -> bool {
        self.focus.popup().is_none() && self.focus.panel() == panel
    }

    pub fn is_popup_focused(&self, popup: PopupFocus) -> bool {
        self.focus.popup() == Some(popup)
    }

    pub fn focus_panel(&mut self, panel: PanelFocus) {
        let panel = if panel == PanelFocus::Log && !self.log_panel.visible {
            PanelFocus::Detail
        } else {
            panel
        };
        self.focus.focus_panel(panel);
    }

    pub fn add_request(&mut self, req: CapturedData) {
        if !self.recording {
            return;
        }

        let was_empty = self.requests.is_empty();
        let tree_entry = request_tree_entry(&req);
        let insert_pos = self
            .requests
            .partition_point(|existing| existing.sequence <= req.sequence);
        let uri = req.uri.clone();
        self.requests.insert(insert_pos, req);
        log::info!("request received: {}", uri);

        if self.request_list.auto_expand {
            self.request_list.open_entry(&tree_entry);
        }
        if was_empty {
            if self.request_list.auto_expand {
                self.request_list.select_entry(&tree_entry);
            } else {
                self.request_list.select_origin(&tree_entry);
            }
        }
    }

    pub fn next(&mut self) {
        let changed = self.request_list.state.key_down();
        self.apply_request_list_change(changed);
    }

    pub fn previous(&mut self) {
        let changed = self.request_list.state.key_up();
        self.apply_request_list_change(changed);
    }

    pub fn selected_request(&self) -> Option<&CapturedData> {
        selected_request_sequence(self.request_list.state.selected())
            .and_then(|sequence| self.requests.iter().find(|req| req.sequence == sequence))
    }

    /// Deletes the selected request leaf or every request under the selected branch.
    ///
    /// Returns the number of captured requests removed. The selected tree path is
    /// resolved against each request's stable tree path, so selecting an origin or
    /// path segment removes the whole visible subtree represented by that node.
    pub fn delete_selected_requests(&mut self) -> usize {
        let selected_path = self.request_list.state.selected().to_vec();
        if selected_path.is_empty() {
            return 0;
        }

        let tree_before_delete = RequestPathTree::from_requests(&self.requests);
        let len_before_deletion = self.requests.len();

        // deletion
        self.requests.retain(|req| {
            let request_path = request_tree_entry(req).request_path();
            !request_path.starts_with(&selected_path)
        });
        let removed_count = len_before_deletion - self.requests.len();

        if removed_count > 0 {
            log::info!("deleted {removed_count} request(s) from request tree");
            self.rebuild_request_list_state_after_delete(&selected_path, &tree_before_delete);
            self.detail_panel.scroll.reset();
        }

        removed_count
    }

    /// Removes all captured requests and resets request-tree UI state.
    ///
    /// Replaces the request vector with a fresh allocation so the memory used by
    /// captured request/response bodies can be released promptly.
    pub fn clear_requests(&mut self) {
        let removed_count = self.requests.len();
        self.requests = Vec::new();
        self.request_list.state = TreeState::default();
        self.detail_panel.scroll.reset();
        if removed_count > 0 {
            log::info!("cleared {removed_count} request(s) from request tree");
        }
    }

    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if is_plain_key(key) && key.code == KeyCode::Char('q') {
            return true;
        }

        if let Some(popup) = self.focus.popup() {
            self.handle_popup_key(popup, key);
            return false;
        }

        if self.handle_focus_key(key) || self.handle_global_panel_key(key) {
            return false;
        }

        match self.focus.panel() {
            PanelFocus::RequestList => self.handle_request_list_key(key),
            PanelFocus::Detail => self.handle_detail_key(key),
            PanelFocus::Log => self.handle_log_key(key),
        }

        false
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::NetworkRequest(req) => self.add_request(req),
            AppEvent::LogMessage(msg) => self.log_panel.add_log(msg),
            AppEvent::CertificateDownloadReady(download_url) => {
                self.certificate_popup.set_download_url(download_url)
            }
        }
    }

    pub fn apply_request_list_change(&mut self, changed: bool) {
        if changed {
            self.detail_panel.scroll.reset();
        }
    }

    fn handle_focus_key(&mut self, key: KeyEvent) -> bool {
        if !is_plain_key(key) && key.modifiers != KeyModifiers::CONTROL {
            return false;
        }

        match key.code {
            KeyCode::Tab if is_plain_key(key) => {
                self.focus_next_panel();
                true
            }
            KeyCode::BackTab if is_plain_key(key) => {
                self.focus_previous_panel();
                true
            }
            KeyCode::Char('h') | KeyCode::Char('H') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Left);
                true
            }
            KeyCode::Char('j') | KeyCode::Char('J') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Down);
                true
            }
            KeyCode::Char('k') | KeyCode::Char('K') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Up);
                true
            }
            KeyCode::Char('l') | KeyCode::Char('L') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Right);
                true
            }
            _ => false,
        }
    }

    fn handle_global_panel_key(&mut self, key: KeyEvent) -> bool {
        if !is_plain_key(key) {
            return false;
        }

        match key.code {
            KeyCode::Char('@') => {
                self.toggle_log_panel();
                true
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.open_certificate_popup();
                true
            }
            _ => false,
        }
    }

    fn handle_popup_key(&mut self, popup: PopupFocus, key: KeyEvent) {
        match popup {
            PopupFocus::Certificate => {
                if is_plain_key(key) && key.code == KeyCode::Esc {
                    self.close_certificate_popup();
                }
            }
        }
    }

    fn handle_request_list_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.next(),
            KeyCode::Char('k') | KeyCode::Up => self.previous(),
            KeyCode::Char('h') | KeyCode::Left => {
                let changed = self.request_list.state.key_left();
                self.apply_request_list_change(changed);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let changed = self.request_list.state.key_right();
                self.apply_request_list_change(changed);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let changed = self.request_list.state.toggle_selected();
                self.apply_request_list_change(changed);
            }
            KeyCode::PageDown => {
                self.request_list.scroll_down();
            }
            KeyCode::PageUp => {
                self.request_list.scroll_up();
            }
            KeyCode::Char('d') => {
                self.delete_selected_requests();
            }
            KeyCode::Char('D') => {
                self.clear_requests();
            }
            _ => {}
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down | KeyCode::PageDown => {
                self.detail_panel.scroll.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up | KeyCode::PageUp => {
                self.detail_panel.scroll.scroll_up();
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.detail_panel.previous_tab();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.detail_panel.next_tab();
            }
            _ => {}
        }
    }

    fn handle_log_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down | KeyCode::PageDown => {
                self.log_panel.scroll.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up | KeyCode::PageUp => {
                self.log_panel.scroll.scroll_up();
            }
            _ => {}
        }
    }

    fn focus_next_panel(&mut self) {
        let panels = focusable_panels(self.log_panel.visible);
        let current_index = panels
            .iter()
            .position(|panel| *panel == self.focus.panel())
            .unwrap_or(0);
        self.focus_panel(panels[(current_index + 1) % panels.len()]);
    }

    fn focus_previous_panel(&mut self) {
        let panels = focusable_panels(self.log_panel.visible);
        let current_index = panels
            .iter()
            .position(|panel| *panel == self.focus.panel())
            .unwrap_or(0);
        self.focus_panel(panels[(current_index + panels.len() - 1) % panels.len()]);
    }

    fn focus_in_direction(&mut self, direction: FocusDirection) {
        if let Some(panel) = focus_neighbor(self.focus.panel(), direction, self.log_panel.visible) {
            self.focus_panel(panel);
        }
    }

    fn toggle_log_panel(&mut self) {
        self.log_panel.toggle();
        self.ensure_focusable_panel();
    }

    fn ensure_focusable_panel(&mut self) {
        if self.focus.panel() == PanelFocus::Log && !self.log_panel.visible {
            self.focus_panel(PanelFocus::Detail);
        }
    }

    fn open_certificate_popup(&mut self) {
        self.certificate_popup.open();
        self.focus.open_popup(PopupFocus::Certificate);
    }

    fn close_certificate_popup(&mut self) {
        self.certificate_popup.close();
        self.focus.close_popup();
        self.ensure_focusable_panel();
    }

    /// Rebuilds request-tree UI state after a delete while preserving valid opens.
    ///
    /// The tree state stores paths independently from the captured request list,
    /// so deleted paths and empty ancestors must be discarded before choosing the
    /// next selection.
    fn rebuild_request_list_state_after_delete(
        &mut self,
        deleted_path: &[String],
        tree_before_delete: &RequestPathTree,
    ) {
        let tree_after_delete = RequestPathTree::from_requests(&self.requests);
        let opened_paths_before_delete = self
            .request_list
            .state
            .opened()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let preserved_opened_paths = opened_paths_before_delete
            .iter()
            .filter(|path| tree_after_delete.contains(path))
            .cloned()
            .collect::<Vec<_>>();
        let visible_paths = tree_after_delete.visible_paths(&preserved_opened_paths);
        let path_to_select = DeleteSelectionContext::new(
            deleted_path,
            tree_before_delete,
            &tree_after_delete,
            &visible_paths,
        )
        .selected_path();

        // Keep the current scroll offset; rendering will adjust it only if the
        // newly selected row falls outside the viewport.
        for opened_path in opened_paths_before_delete {
            if !tree_after_delete.contains(&opened_path) {
                self.request_list.state.close(&opened_path);
            }
        }
        self.request_list.state.select(path_to_select);
    }
}

fn focusable_panels(log_visible: bool) -> &'static [PanelFocus] {
    if log_visible {
        &FOCUSABLE_PANELS_WITH_LOG
    } else {
        &FOCUSABLE_PANELS_WITHOUT_LOG
    }
}

fn focus_neighbor(
    current: PanelFocus,
    direction: FocusDirection,
    log_visible: bool,
) -> Option<PanelFocus> {
    match (current, direction) {
        (PanelFocus::RequestList, FocusDirection::Right) => Some(PanelFocus::Detail),
        (PanelFocus::Detail, FocusDirection::Left) => Some(PanelFocus::RequestList),
        (PanelFocus::Detail, FocusDirection::Down) if log_visible => Some(PanelFocus::Log),
        (PanelFocus::Log, FocusDirection::Left) => Some(PanelFocus::RequestList),
        (PanelFocus::Log, FocusDirection::Up) => Some(PanelFocus::Detail),
        _ => None,
    }
}

fn is_plain_key(key: KeyEvent) -> bool {
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}

#[derive(Debug, Default)]
struct RequestPathTree {
    roots: Vec<RequestTreePathNode>,
}

impl RequestPathTree {
    /// Builds an internal path-only tree from captured requests.
    ///
    /// This tree mirrors the UI tree's identifier structure without depending on UI
    /// rendering code, which lets app-state logic make selection decisions directly.
    fn from_requests(requests: &[CapturedData]) -> Self {
        let mut tree = Self::default();

        for req in requests {
            tree.insert_request_path(request_tree_entry(req).request_path());
        }

        tree
    }

    /// Inserts one request path into the internal request tree.
    fn insert_request_path(&mut self, path: Vec<String>) {
        let Some(origin_identifier) = path.first() else {
            return;
        };

        let mut current = self.root_mut_or_insert(vec![origin_identifier.clone()]);
        for identifier in path.iter().skip(1).take(path.len().saturating_sub(2)) {
            let mut branch_path = current.path.clone();
            branch_path.push(identifier.clone());
            current = current.branch_child_mut_or_insert(branch_path);
        }

        if let Some(request_identifier) = path.last() {
            let mut request_path = current.path.clone();
            request_path.push(request_identifier.clone());
            current
                .children
                .push(RequestTreePathNode::new(request_path));
        }
    }

    /// Returns an existing root node or appends a new one in capture order.
    fn root_mut_or_insert(&mut self, path: Vec<String>) -> &mut RequestTreePathNode {
        if let Some(index) = self.roots.iter().position(|root| root.path == path) {
            return &mut self.roots[index];
        }

        self.roots.push(RequestTreePathNode::new(path));
        self.roots
            .last_mut()
            .expect("root was inserted immediately before access")
    }

    /// Returns the tree paths that are currently visible for the preserved open paths.
    ///
    /// Folded branches are included as visible rows, while their descendants are not.
    fn visible_paths(&self, opened_paths: &[Vec<String>]) -> Vec<Vec<String>> {
        let opened_paths = opened_paths.iter().cloned().collect::<HashSet<_>>();
        let mut visible_paths = Vec::new();
        RequestTreePathNode::collect_visible_paths(&self.roots, &opened_paths, &mut visible_paths);
        visible_paths
    }

    /// Returns true when the exact tree path exists in the internal tree.
    fn contains(&self, path: &[String]) -> bool {
        self.find(path).is_some()
    }

    /// Finds a node by following its full identifier path from the tree roots.
    fn find(&self, path: &[String]) -> Option<&RequestTreePathNode> {
        RequestTreePathNode::find_in(&self.roots, path)
    }

    /// Returns the previous and next siblings of a path in the internal tree.
    fn neighboring_paths(&self, path: &[String]) -> (Option<Vec<String>>, Option<Vec<String>>) {
        if path.is_empty() {
            return (None, None);
        }

        let siblings = if path.len() == 1 {
            self.roots.as_slice()
        } else {
            self.find(&path[..path.len() - 1])
                .map(|parent| parent.children.as_slice())
                .unwrap_or(&[])
        };

        let Some(index) = siblings.iter().position(|sibling| sibling.path == path) else {
            return (None, None);
        };

        let previous = index
            .checked_sub(1)
            .and_then(|index| siblings.get(index))
            .map(|sibling| sibling.path.clone());
        let next = siblings.get(index + 1).map(|sibling| sibling.path.clone());

        (previous, next)
    }
}

#[derive(Clone, Debug)]
struct RequestTreePathNode {
    path: Vec<String>,
    children: Vec<RequestTreePathNode>,
}

impl RequestTreePathNode {
    /// Creates an internal request-tree node identified by its full tree path.
    fn new(path: Vec<String>) -> Self {
        Self {
            path,
            children: Vec::new(),
        }
    }

    /// Returns an existing branch child or inserts it before any request leaves.
    ///
    /// This mirrors the UI tree ordering where path branches are grouped before
    /// leaf requests that live directly under the same parent.
    fn branch_child_mut_or_insert(&mut self, path: Vec<String>) -> &mut Self {
        if let Some(index) = self.children.iter().position(|child| child.path == path) {
            return &mut self.children[index];
        }

        let insert_index = self
            .children
            .iter()
            .position(RequestTreePathNode::is_request_leaf)
            .unwrap_or(self.children.len());
        self.children.insert(insert_index, Self::new(path));
        &mut self.children[insert_index]
    }

    /// Returns true when this node represents a captured request leaf.
    fn is_request_leaf(&self) -> bool {
        self.path
            .last()
            .is_some_and(|identifier| identifier.starts_with(REQUEST_IDENTIFIER_PREFIX))
    }

    /// Walks the internal tree in UI order and collects rows visible under open branches.
    fn collect_visible_paths(
        nodes: &[Self],
        opened_paths: &HashSet<Vec<String>>,
        visible_paths: &mut Vec<Vec<String>>,
    ) {
        for node in nodes {
            visible_paths.push(node.path.clone());
            if opened_paths.contains(&node.path) {
                Self::collect_visible_paths(&node.children, opened_paths, visible_paths);
            }
        }
    }

    /// Finds a node by following its full identifier path from the given nodes.
    fn find_in<'a>(nodes: &'a [Self], path: &[String]) -> Option<&'a Self> {
        let (identifier, rest) = path.split_first()?;
        let node = nodes
            .iter()
            .find(|node| node.path.last() == Some(identifier))?;

        if rest.is_empty() {
            Some(node)
        } else {
            Self::find_in(&node.children, rest)
        }
    }
}

struct DeleteSelectionContext<'a> {
    deleted_path: &'a [String],
    tree_before_delete: &'a RequestPathTree,
    tree_after_delete: &'a RequestPathTree,
    visible_paths: &'a [Vec<String>],
}

impl<'a> DeleteSelectionContext<'a> {
    /// Creates a chooser for the next selected row after a delete operation.
    fn new(
        deleted_path: &'a [String],
        tree_before_delete: &'a RequestPathTree,
        tree_after_delete: &'a RequestPathTree,
        visible_paths: &'a [Vec<String>],
    ) -> Self {
        Self {
            deleted_path,
            tree_before_delete,
            tree_after_delete,
            visible_paths,
        }
    }

    /// Picks the row that should be selected after the currently selected path is deleted.
    ///
    /// Preference is given to the previous sibling's deepest visible descendant,
    /// then the next visible sibling, then the first visible row. This keeps the
    /// selection anchored near the user's deletion point and respects folded nodes.
    fn selected_path(&self) -> Vec<String> {
        if let Some(deleted_start_path) = self.first_removed_prefix() {
            let (previous_sibling, next_sibling) = self
                .tree_before_delete
                .neighboring_paths(&deleted_start_path);

            if let Some(previous_sibling) =
                previous_sibling.filter(|path| self.tree_after_delete.contains(path))
            {
                if let Some(visible_descendant) = self.deepest_visible_descendant(&previous_sibling)
                {
                    return visible_descendant;
                }
            }

            if let Some(next_sibling) =
                next_sibling.filter(|path| self.tree_after_delete.contains(path))
            {
                if self.visible_paths.contains(&next_sibling) {
                    return next_sibling;
                }
            }
        }

        self.visible_paths.first().cloned().unwrap_or_default()
    }

    /// Finds the shallowest selected prefix that disappeared as a result of deletion.
    ///
    /// Deleting a branch can remove empty ancestors too; this identifies the start
    /// node whose previous/next siblings should drive the replacement selection.
    fn first_removed_prefix(&self) -> Option<Vec<String>> {
        (1..=self.deleted_path.len())
            .map(|len| self.deleted_path[..len].to_vec())
            .find(|prefix| {
                self.tree_before_delete.contains(prefix) && !self.tree_after_delete.contains(prefix)
            })
    }

    /// Returns the deepest visible row under the given path.
    fn deepest_visible_descendant(&self, path: &[String]) -> Option<Vec<String>> {
        self.visible_paths
            .iter()
            .rev()
            .find(|visible_path| visible_path.starts_with(path))
            .cloned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestTreeEntry {
    pub origin: String,
    pub segments: Vec<String>,
    request_sequence: u64,
}

impl RequestTreeEntry {
    pub fn request_identifier(&self) -> String {
        request_identifier(self.request_sequence)
    }

    pub fn request_path(&self) -> Vec<String> {
        let mut path = self.parent_path();
        path.push(self.request_identifier());
        path
    }

    fn branch_paths(&self) -> Vec<Vec<String>> {
        let mut branches = Vec::new();
        let mut path = vec![origin_identifier(&self.origin)];
        branches.push(path.clone());

        for segment in parent_segments(&self.segments) {
            path.push(segment_identifier(segment));
            branches.push(path.clone());
        }

        branches
    }

    fn parent_path(&self) -> Vec<String> {
        let mut path = vec![origin_identifier(&self.origin)];
        for segment in parent_segments(&self.segments) {
            path.push(segment_identifier(segment));
        }
        path
    }
}

pub fn request_tree_entry(req: &CapturedData) -> RequestTreeEntry {
    let (origin, segments) = Url::parse(&req.uri)
        .ok()
        .filter(Url::has_host)
        .map(|url| {
            (
                absolute_url_origin(&url),
                path_segments_with_query(url.path(), url.query()),
            )
        })
        .unwrap_or_else(|| fallback_origin_and_segments(req));

    RequestTreeEntry {
        origin,
        segments,
        request_sequence: req.sequence,
    }
}

fn selected_request_sequence(path: &[String]) -> Option<u64> {
    path.last()?
        .strip_prefix(REQUEST_IDENTIFIER_PREFIX)?
        .parse()
        .ok()
}

fn origin_identifier(origin: &str) -> String {
    format!("{ORIGIN_IDENTIFIER_PREFIX}{origin}")
}

fn segment_identifier(segment: &str) -> String {
    format!("{SEGMENT_IDENTIFIER_PREFIX}{segment}")
}

fn request_identifier(sequence: u64) -> String {
    format!("{REQUEST_IDENTIFIER_PREFIX}{sequence}")
}

fn parent_segments(segments: &[String]) -> &[String] {
    segments
        .split_last()
        .map_or(&[], |(_, parent_segments)| parent_segments)
}

fn absolute_url_origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or("(unknown host)");
    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

fn fallback_origin_and_segments(req: &CapturedData) -> (String, Vec<String>) {
    let origin = request_header(req, "host")
        .map(|host| format!("https://{host}"))
        .unwrap_or_else(|| "(unknown host)".to_string());
    let (path, query) = req
        .uri
        .split_once('?')
        .map_or((req.uri.as_str(), None), |(path, query)| {
            (path, Some(query))
        });

    (origin, path_segments_with_query(path, query))
}

fn request_header<'a>(req: &'a CapturedData, name: &str) -> Option<&'a str> {
    req.req_headers
        .iter()
        .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.is_empty())
        .map(|(_, value)| value.as_str())
}

fn path_segments_with_query(path: &str, query: Option<&str>) -> Vec<String> {
    let mut segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();

    match (segments.last_mut(), query) {
        (Some(last), Some(query)) => {
            last.push('?');
            last.push_str(query);
        }
        (None, Some(query)) => segments.push(format!("?{query}")),
        (None, None) => segments.push("/".to_string()),
        (Some(_), None) => {}
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{settings::RequestListSettings, ui::RootView};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use http::Method;
    use ratatui::{Terminal, backend::TestBackend};

    fn ui_settings(auto_expand: bool) -> UiSettings {
        UiSettings {
            request_list: RequestListSettings { auto_expand },
        }
    }

    fn captured(uri: &str) -> CapturedData {
        captured_with_sequence(0, uri)
    }

    fn captured_with_sequence(sequence: u64, uri: &str) -> CapturedData {
        CapturedData {
            id: uuid::Uuid::nil(),
            sequence,
            method: Method::GET,
            uri: uri.to_string(),
            status: None,
            req_headers: vec![("host".to_string(), "fallback.example.com".to_string())],
            res_headers: vec![],
            req_body: None,
            res_body: None,
        }
    }

    fn tree_path(identifiers: &[&str]) -> Vec<String> {
        identifiers
            .iter()
            .map(|identifier| (*identifier).to_string())
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn ctrl_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn render_app(app: &mut App) {
        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut ui = RootView::new();

        terminal
            .draw(|frame| ui.render(frame, app))
            .expect("request tree should render in tests");
    }

    #[test]
    fn focus_starts_on_request_list() {
        let app = App::new(ui_settings(true));

        assert_eq!(app.focused_panel(), PanelFocus::RequestList);
        assert_eq!(app.focused_popup(), None);
        assert!(app.is_panel_focused(PanelFocus::RequestList));
    }

    #[test]
    fn tab_cycles_focus_through_visible_panels() {
        let mut app = App::new(ui_settings(true));

        assert!(!app.handle_key_event(key(KeyCode::Tab)));
        assert_eq!(app.focused_panel(), PanelFocus::Detail);

        assert!(!app.handle_key_event(key(KeyCode::Tab)));
        assert_eq!(app.focused_panel(), PanelFocus::Log);

        assert!(!app.handle_key_event(key(KeyCode::Tab)));
        assert_eq!(app.focused_panel(), PanelFocus::RequestList);

        app.log_panel.visible = false;
        assert!(!app.handle_key_event(key(KeyCode::Tab)));
        assert_eq!(app.focused_panel(), PanelFocus::Detail);

        assert!(!app.handle_key_event(key(KeyCode::Tab)));
        assert_eq!(app.focused_panel(), PanelFocus::RequestList);
    }

    #[test]
    fn directional_focus_uses_semantic_panel_graph() {
        let mut app = App::new(ui_settings(true));

        app.handle_key_event(ctrl_key(KeyCode::Char('l')));
        assert_eq!(app.focused_panel(), PanelFocus::Detail);

        app.handle_key_event(ctrl_key(KeyCode::Char('j')));
        assert_eq!(app.focused_panel(), PanelFocus::Log);

        app.handle_key_event(ctrl_key(KeyCode::Char('k')));
        assert_eq!(app.focused_panel(), PanelFocus::Detail);

        app.handle_key_event(ctrl_key(KeyCode::Char('h')));
        assert_eq!(app.focused_panel(), PanelFocus::RequestList);
    }

    #[test]
    fn hiding_focused_log_panel_moves_focus_to_detail() {
        let mut app = App::new(ui_settings(true));

        app.focus_panel(PanelFocus::Log);
        app.handle_key_event(key(KeyCode::Char('@')));

        assert!(!app.log_panel.visible);
        assert_eq!(app.focused_panel(), PanelFocus::Detail);
    }

    #[test]
    fn certificate_popup_takes_modal_focus_until_escape() {
        let mut app = App::new(ui_settings(true));

        app.focus_panel(PanelFocus::Log);
        app.handle_key_event(key(KeyCode::Char('c')));

        assert!(app.certificate_popup.visible);
        assert_eq!(app.focused_popup(), Some(PopupFocus::Certificate));
        assert!(!app.is_panel_focused(PanelFocus::Log));

        app.handle_key_event(ctrl_key(KeyCode::Char('h')));
        assert_eq!(app.focused_panel(), PanelFocus::Log);
        assert_eq!(app.focused_popup(), Some(PopupFocus::Certificate));

        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.certificate_popup.visible);
        assert_eq!(app.focused_popup(), None);
        assert!(app.is_panel_focused(PanelFocus::Log));
    }

    #[test]
    fn detail_focus_uses_h_l_to_switch_tabs() {
        let mut app = App::new(ui_settings(true));

        app.focus_panel(PanelFocus::Detail);
        app.handle_key_event(key(KeyCode::Char('h')));
        assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseBody);

        app.handle_key_event(key(KeyCode::Char('l')));
        assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestHeader);

        app.handle_key_event(key(KeyCode::Char('l')));
        assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
    }

    #[test]
    fn panel_keys_apply_only_to_focused_panel() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
        app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
        let selected_before = app.request_list.state.selected().to_vec();
        app.detail_panel.scroll.max_offset = 5;
        app.focus_panel(PanelFocus::Detail);

        app.handle_key_event(key(KeyCode::Char('j')));

        assert_eq!(app.request_list.state.selected(), selected_before);
        assert_eq!(app.detail_panel.scroll.offset, 1);

        app.focus_panel(PanelFocus::RequestList);
        app.handle_key_event(key(KeyCode::Char('j')));

        assert_ne!(app.request_list.state.selected(), selected_before);
    }

    #[test]
    fn request_tree_entry_splits_absolute_url() {
        let entry = request_tree_entry(&captured_with_sequence(
            7,
            "https://some.host.com/api/v1/getUserInfo?a=1&b=2",
        ));

        assert_eq!(entry.origin, "https://some.host.com");
        assert_eq!(entry.segments, ["api", "v1", "getUserInfo?a=1&b=2"]);
        assert_eq!(
            entry.request_path(),
            [
                "origin:https://some.host.com",
                "segment:api",
                "segment:v1",
                "request:7"
            ]
        );
    }

    #[test]
    fn request_tree_entry_uses_host_header_for_origin_form_uri() {
        let entry = request_tree_entry(&captured("/common/getSomeOtherInfo"));

        assert_eq!(entry.origin, "https://fallback.example.com");
        assert_eq!(entry.segments, ["common", "getSomeOtherInfo"]);
    }

    #[test]
    fn requests_are_ordered_by_capture_sequence() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(1, "https://a.com/b"));
        app.add_request(captured_with_sequence(0, "https://a.com/a"));

        let uris = app
            .requests
            .iter()
            .map(|req| req.uri.as_str())
            .collect::<Vec<_>>();

        assert_eq!(uris, ["https://a.com/a", "https://a.com/b"]);

        app.request_list.state.select(vec![
            "origin:https://a.com".to_string(),
            "request:1".to_string(),
        ]);

        assert_eq!(
            app.selected_request().map(|req| req.uri.as_str()),
            Some("https://a.com/b")
        );
    }

    #[test]
    fn request_list_branches_stay_folded_by_default() {
        let mut app = App::new(ui_settings(false));

        app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

        assert!(app.request_list.state.opened().is_empty());
        assert_eq!(app.request_list.state.selected().len(), 1);
        assert_eq!(
            app.request_list.state.selected()[0],
            "origin:https://some.host.com"
        );
        assert!(app.selected_request().is_none());
    }

    #[test]
    fn request_list_auto_expand_opens_new_request_branches() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

        assert!(
            app.request_list
                .state
                .opened()
                .contains(&vec!["origin:https://some.host.com".to_string()])
        );
        assert!(app.request_list.state.opened().contains(&vec![
            "origin:https://some.host.com".to_string(),
            "segment:api".to_string()
        ]));
        assert!(app.request_list.state.opened().contains(&vec![
            "origin:https://some.host.com".to_string(),
            "segment:api".to_string(),
            "segment:v1".to_string()
        ]));
        assert_eq!(
            app.selected_request().map(|req| req.uri.as_str()),
            Some("https://some.host.com/api/v1/getUserInfo?a=1")
        );
    }

    #[test]
    fn request_list_preserves_user_opened_branches_when_auto_expand_is_disabled() {
        let mut app = App::new(ui_settings(false));
        let origin = "origin:https://some.host.com".to_string();
        let api = "segment:api".to_string();
        let v1 = "segment:v1".to_string();

        app.add_request(captured("https://some.host.com/api/first"));
        app.request_list.state.open(vec![origin.clone()]);
        app.request_list
            .state
            .open(vec![origin.clone(), api.clone()]);
        app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

        assert!(
            app.request_list
                .state
                .opened()
                .contains(&vec![origin.clone(), api.clone()])
        );
        assert!(
            !app.request_list
                .state
                .opened()
                .contains(&vec![origin, api, v1])
        );
    }

    #[test]
    fn delete_selected_requests_removes_leaf_request() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
        app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
        app.request_list.state.select(vec![
            "origin:https://some.host.com".to_string(),
            "segment:api".to_string(),
            "request:0".to_string(),
        ]);

        assert_eq!(app.delete_selected_requests(), 1);

        assert_eq!(app.requests.len(), 1);
        assert_eq!(app.requests[0].uri, "https://some.host.com/api/b");
        assert_eq!(
            app.request_list.state.selected(),
            [
                "origin:https://some.host.com".to_string(),
                "segment:api".to_string(),
                "request:1".to_string()
            ]
        );
        assert_eq!(
            app.selected_request().map(|req| req.uri.as_str()),
            Some("https://some.host.com/api/b")
        );
    }

    #[test]
    fn delete_selected_requests_removes_subtree_requests() {
        let mut app = App::new(ui_settings(true));
        let origin = "origin:https://some.host.com".to_string();
        let api = "segment:api".to_string();
        let v1 = "segment:v1".to_string();

        app.add_request(captured_with_sequence(0, "https://some.host.com/api/v1/a"));
        app.add_request(captured_with_sequence(1, "https://some.host.com/api/v1/b"));
        app.add_request(captured_with_sequence(2, "https://some.host.com/api/v2/c"));
        app.request_list
            .state
            .select(vec![origin.clone(), api.clone(), v1.clone()]);

        assert_eq!(app.delete_selected_requests(), 2);

        assert_eq!(app.requests.len(), 1);
        assert_eq!(app.requests[0].uri, "https://some.host.com/api/v2/c");
        assert_eq!(
            app.request_list.state.selected(),
            [origin.clone(), api.clone(), "segment:v2".to_string()]
        );
        assert!(
            app.request_list
                .state
                .opened()
                .contains(&vec![origin.clone(), api.clone()])
        );
        assert!(
            !app.request_list
                .state
                .opened()
                .contains(&vec![origin, api, v1])
        );
    }

    #[test]
    fn delete_selected_requests_selects_previous_leaf_sibling() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
        app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
        app.add_request(captured_with_sequence(2, "https://some.host.com/api/c"));
        app.request_list.state.select(tree_path(&[
            "origin:https://some.host.com",
            "segment:api",
            "request:1",
        ]));

        assert_eq!(app.delete_selected_requests(), 1);

        assert_eq!(
            app.request_list.state.selected(),
            tree_path(&["origin:https://some.host.com", "segment:api", "request:0"])
        );
        assert_eq!(
            app.selected_request().map(|req| req.uri.as_str()),
            Some("https://some.host.com/api/a")
        );
    }

    #[test]
    fn delete_selected_requests_selects_next_root_when_first_branch_disappears() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
        app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
        app.add_request(captured_with_sequence(2, "https://b.com/api4"));
        app.request_list
            .state
            .select(tree_path(&["origin:https://a.com", "segment:some"]));

        assert_eq!(app.delete_selected_requests(), 2);

        assert_eq!(
            app.request_list.state.selected(),
            tree_path(&["origin:https://b.com"])
        );
        assert_eq!(app.requests.len(), 1);
        assert_eq!(app.requests[0].uri, "https://b.com/api4");
    }

    #[test]
    fn delete_selected_requests_selects_deepest_visible_node_in_previous_root() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
        app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
        app.add_request(captured_with_sequence(2, "https://a.com/some/path/api3"));
        app.add_request(captured_with_sequence(3, "https://b.com/api4"));
        app.request_list
            .state
            .select(tree_path(&["origin:https://b.com", "request:3"]));

        assert_eq!(app.delete_selected_requests(), 1);

        assert_eq!(
            app.request_list.state.selected(),
            tree_path(&[
                "origin:https://a.com",
                "segment:some",
                "segment:path",
                "request:2"
            ])
        );
        assert_eq!(
            app.selected_request().map(|req| req.uri.as_str()),
            Some("https://a.com/some/path/api3")
        );
    }

    #[test]
    fn delete_selected_requests_selects_folded_previous_branch_node() {
        let mut app = App::new(ui_settings(true));
        let some_path = tree_path(&["origin:https://a.com", "segment:some"]);

        app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
        app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
        app.add_request(captured_with_sequence(2, "https://a.com/some/path/api3"));
        app.add_request(captured_with_sequence(3, "https://b.com/api4"));
        app.request_list.state.close(&some_path);
        app.request_list
            .state
            .select(tree_path(&["origin:https://b.com", "request:3"]));

        assert_eq!(app.delete_selected_requests(), 1);

        assert_eq!(app.request_list.state.selected(), some_path);
        assert!(app.selected_request().is_none());
    }

    #[test]
    fn delete_selected_requests_preserves_scroll_when_new_selection_is_visible() {
        let mut app = App::new(ui_settings(true));

        for sequence in 0..30 {
            app.add_request(captured_with_sequence(
                sequence,
                &format!("https://some.host.com/api/item{sequence}"),
            ));
        }

        render_app(&mut app);
        for _ in 0..8 {
            app.request_list.scroll_down();
        }
        render_app(&mut app);
        let offset_before_delete = app.request_list.state.get_offset();
        app.request_list.state.select(tree_path(&[
            "origin:https://some.host.com",
            "segment:api",
            "request:10",
        ]));

        assert_eq!(app.delete_selected_requests(), 1);
        render_app(&mut app);

        assert_eq!(app.request_list.state.get_offset(), offset_before_delete);
        assert_eq!(
            app.request_list.state.selected(),
            tree_path(&["origin:https://some.host.com", "segment:api", "request:9"])
        );
    }

    #[test]
    fn clear_requests_drops_records_and_resets_tree_state() {
        let mut app = App::new(ui_settings(true));

        app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
        app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
        app.detail_panel.scroll.offset = 1;
        assert!(app.requests.capacity() > 0);

        app.clear_requests();

        assert!(app.requests.is_empty());
        assert_eq!(app.requests.capacity(), 0);
        assert!(app.request_list.state.selected().is_empty());
        assert!(app.request_list.state.opened().is_empty());
        assert_eq!(app.detail_panel.scroll.offset, 0);
    }
}
