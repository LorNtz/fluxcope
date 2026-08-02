use crossterm::event::{KeyCode, KeyEvent};
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectItemRole {
    Value,
    Action,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectFilterMode {
    Normal,
    AlwaysVisible,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectItem<'a, Id> {
    pub id: Id,
    pub label: Cow<'a, str>,
    pub role: SelectItemRole,
    pub filter_mode: SelectFilterMode,
}

impl<'a, Id> SelectItem<'a, Id> {
    pub(crate) fn value(id: Id, label: impl Into<Cow<'a, str>>) -> Self {
        Self {
            id,
            label: label.into(),
            role: SelectItemRole::Value,
            filter_mode: SelectFilterMode::Normal,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectCommit<Id> {
    pub id: Id,
    pub role: SelectItemRole,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SelectOutcome<Id> {
    None,
    Opened,
    Closed,
    Committed(SelectCommit<Id>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SelectResolvedItems {
    indices: Vec<usize>,
}

impl SelectResolvedItems {
    pub(crate) fn len(&self) -> usize {
        self.indices.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    pub(crate) fn item_index(&self, filtered_index: usize) -> Option<usize> {
        self.indices.get(filtered_index).copied()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SelectState {
    open: bool,
    filter: String,
    filter_cursor: usize,
    focused_filtered_index: usize,
    scroll_offset: usize,
}

impl SelectState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open
    }

    pub(crate) fn filter(&self) -> &str {
        &self.filter
    }

    pub(crate) fn filter_prefix(&self) -> &str {
        let end = byte_index_for_char(&self.filter, self.filter_cursor);
        &self.filter[..end]
    }

    pub(crate) fn focused_filtered_index(&self) -> usize {
        self.focused_filtered_index
    }

    pub(crate) fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    pub(crate) fn open_with_selected<Id: PartialEq>(
        &mut self,
        items: &[SelectItem<'_, Id>],
        selected: Option<&Id>,
        max_visible_items: usize,
    ) {
        self.open = true;
        self.filter.clear();
        self.filter_cursor = 0;
        self.focused_filtered_index = Self::selected_item_index(items, selected).unwrap_or(0);
        self.scroll_offset = 0;
        self.clamp_focus(items);
        self.ensure_focus_visible(max_visible_items);
    }

    pub(crate) fn close(&mut self) {
        self.open = false;
        self.filter.clear();
        self.filter_cursor = 0;
        self.focused_filtered_index = 0;
        self.scroll_offset = 0;
    }

    pub(crate) fn resolve_items<Id>(&self, items: &[SelectItem<'_, Id>]) -> SelectResolvedItems {
        let indices = if self.filter.is_empty() {
            (0..items.len()).collect()
        } else {
            let filter = self.filter.to_lowercase();
            items
                .iter()
                .enumerate()
                .filter_map(|(index, item)| {
                    let visible = item.filter_mode == SelectFilterMode::AlwaysVisible
                        || item.label.to_lowercase().contains(&filter);
                    visible.then_some(index)
                })
                .collect()
        };

        SelectResolvedItems { indices }
    }

    pub(crate) fn focused_item_index<Id>(&self, items: &[SelectItem<'_, Id>]) -> Option<usize> {
        self.resolve_items(items)
            .item_index(self.focused_filtered_index)
    }

    pub(crate) fn selected_item_index<Id: PartialEq>(
        items: &[SelectItem<'_, Id>],
        selected: Option<&Id>,
    ) -> Option<usize> {
        selected.and_then(|selected| items.iter().position(|item| &item.id == selected))
    }

    pub(crate) fn commit_focused<Id: Clone>(
        &mut self,
        items: &[SelectItem<'_, Id>],
    ) -> Option<SelectCommit<Id>> {
        let item_index = self.focused_item_index(items)?;
        self.commit_item(items, item_index)
    }

    pub(crate) fn commit_filtered_index<Id: Clone>(
        &mut self,
        items: &[SelectItem<'_, Id>],
        filtered_index: usize,
    ) -> Option<SelectCommit<Id>> {
        let item_index = self.resolve_items(items).item_index(filtered_index)?;
        self.commit_item(items, item_index)
    }

    pub(crate) fn handle_key<Id: Clone>(
        &mut self,
        key: KeyEvent,
        items: &[SelectItem<'_, Id>],
        max_visible_items: usize,
    ) -> SelectOutcome<Id> {
        if !self.open {
            if key.code == KeyCode::Enter {
                self.open = true;
                self.clamp_focus(items);
                self.ensure_focus_visible(max_visible_items);
                return SelectOutcome::Opened;
            }
            return SelectOutcome::None;
        }

        match key.code {
            KeyCode::Esc => {
                self.close();
                SelectOutcome::Closed
            }
            KeyCode::Enter => match self.commit_focused(items) {
                Some(commit) => SelectOutcome::Committed(commit),
                None => SelectOutcome::None,
            },
            KeyCode::Up => {
                self.move_previous(items, max_visible_items);
                SelectOutcome::None
            }
            KeyCode::Down => {
                self.move_next(items, max_visible_items);
                SelectOutcome::None
            }
            KeyCode::PageUp => {
                self.move_page_up(items, max_visible_items);
                SelectOutcome::None
            }
            KeyCode::PageDown => {
                self.move_page_down(items, max_visible_items);
                SelectOutcome::None
            }
            KeyCode::Backspace => {
                self.backspace(items);
                SelectOutcome::None
            }
            KeyCode::Left => {
                self.filter_cursor = self.filter_cursor.saturating_sub(1);
                SelectOutcome::None
            }
            KeyCode::Right => {
                self.filter_cursor = self.filter_cursor.saturating_add(1).min(self.filter_len());
                SelectOutcome::None
            }
            KeyCode::Char(ch) => {
                self.insert_char(ch, items);
                SelectOutcome::None
            }
            _ => SelectOutcome::None,
        }
    }

    pub(crate) fn scroll_up<Id>(&mut self, items: &[SelectItem<'_, Id>], max_visible_items: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
        if self.focused_filtered_index >= self.scroll_offset.saturating_add(max_visible_items) {
            self.focused_filtered_index = self
                .scroll_offset
                .saturating_add(max_visible_items)
                .saturating_sub(1);
        }
        self.clamp_focus(items);
    }

    pub(crate) fn scroll_down<Id>(
        &mut self,
        items: &[SelectItem<'_, Id>],
        max_visible_items: usize,
    ) {
        let filtered_count = self.resolve_items(items).len();
        let max_scroll = filtered_count.saturating_sub(max_visible_items);
        self.scroll_offset = self.scroll_offset.saturating_add(1).min(max_scroll);
        if self.focused_filtered_index < self.scroll_offset {
            self.focused_filtered_index = self.scroll_offset;
        }
        self.clamp_focus(items);
    }

    fn commit_item<Id: Clone>(
        &mut self,
        items: &[SelectItem<'_, Id>],
        item_index: usize,
    ) -> Option<SelectCommit<Id>> {
        let item = items.get(item_index)?;
        Some(SelectCommit {
            id: item.id.clone(),
            role: item.role,
        })
    }

    fn move_previous<Id>(&mut self, items: &[SelectItem<'_, Id>], max_visible_items: usize) {
        self.focused_filtered_index = self.focused_filtered_index.saturating_sub(1);
        self.clamp_focus(items);
        self.ensure_focus_visible(max_visible_items);
    }

    fn move_next<Id>(&mut self, items: &[SelectItem<'_, Id>], max_visible_items: usize) {
        self.focused_filtered_index = self.focused_filtered_index.saturating_add(1);
        self.clamp_focus(items);
        self.ensure_focus_visible(max_visible_items);
    }

    fn move_page_up<Id>(&mut self, items: &[SelectItem<'_, Id>], max_visible_items: usize) {
        let step = max_visible_items.max(1);
        self.focused_filtered_index = self.focused_filtered_index.saturating_sub(step);
        self.clamp_focus(items);
        self.ensure_focus_visible(max_visible_items);
    }

    fn move_page_down<Id>(&mut self, items: &[SelectItem<'_, Id>], max_visible_items: usize) {
        let step = max_visible_items.max(1);
        self.focused_filtered_index = self.focused_filtered_index.saturating_add(step);
        self.clamp_focus(items);
        self.ensure_focus_visible(max_visible_items);
    }

    fn insert_char<Id>(&mut self, ch: char, items: &[SelectItem<'_, Id>]) {
        let index = byte_index_for_char(&self.filter, self.filter_cursor);
        self.filter.insert(index, ch);
        self.filter_cursor += 1;
        self.reset_filter_position(items);
    }

    fn backspace<Id>(&mut self, items: &[SelectItem<'_, Id>]) {
        if self.filter_cursor == 0 {
            return;
        }

        let remove_start = byte_index_for_char(&self.filter, self.filter_cursor - 1);
        let remove_end = byte_index_for_char(&self.filter, self.filter_cursor);
        self.filter.replace_range(remove_start..remove_end, "");
        self.filter_cursor -= 1;
        self.reset_filter_position(items);
    }

    fn reset_filter_position<Id>(&mut self, items: &[SelectItem<'_, Id>]) {
        self.focused_filtered_index = 0;
        self.scroll_offset = 0;
        self.clamp_focus(items);
    }

    fn clamp_focus<Id>(&mut self, items: &[SelectItem<'_, Id>]) {
        let filtered_count = self.resolve_items(items).len();
        if filtered_count == 0 {
            self.focused_filtered_index = 0;
            self.scroll_offset = 0;
        } else {
            self.focused_filtered_index = self.focused_filtered_index.min(filtered_count - 1);
            self.scroll_offset = self.scroll_offset.min(filtered_count - 1);
        }
    }

    fn ensure_focus_visible(&mut self, max_visible_items: usize) {
        let max_visible_items = max_visible_items.max(1);
        if self.focused_filtered_index < self.scroll_offset {
            self.scroll_offset = self.focused_filtered_index;
        } else if self.focused_filtered_index
            >= self.scroll_offset.saturating_add(max_visible_items)
        {
            self.scroll_offset = self
                .focused_filtered_index
                .saturating_add(1)
                .saturating_sub(max_visible_items);
        }
    }

    fn filter_len(&self) -> usize {
        self.filter.chars().count()
    }
}

fn byte_index_for_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn items() -> Vec<SelectItem<'static, usize>> {
        vec![
            SelectItem::value(0, "dev"),
            SelectItem::value(1, "qa"),
            SelectItem::value(2, "prod"),
        ]
    }

    #[test]
    fn open_focuses_selected_item() {
        let items = items();
        let mut state = SelectState::new();

        state.open_with_selected(&items, Some(&2), 4);

        assert!(state.is_open());
        assert_eq!(state.focused_filtered_index(), 2);
        assert_eq!(state.focused_item_index(&items), Some(2));
    }

    #[test]
    fn filtering_resets_focus_to_first_match() {
        let items = items();
        let mut state = SelectState::new();
        state.open_with_selected(&items, Some(&2), 4);

        state.handle_key(key(KeyCode::Char('d')), &items, 4);

        assert_eq!(state.filter(), "d");
        assert_eq!(state.focused_filtered_index(), 0);
        assert_eq!(state.focused_item_index(&items), Some(0));
    }

    #[test]
    fn commit_returns_item_id_and_role() {
        let items = items();
        let mut state = SelectState::new();
        state.open_with_selected(&items, Some(&0), 4);
        state.handle_key(key(KeyCode::Down), &items, 4);

        let outcome = state.handle_key(key(KeyCode::Enter), &items, 4);

        assert_eq!(
            outcome,
            SelectOutcome::Committed(SelectCommit {
                id: 1,
                role: SelectItemRole::Value
            })
        );
        assert!(state.is_open());
    }

    #[test]
    fn action_items_can_stay_visible_when_filtered() {
        let items = vec![
            SelectItem::value(0, "dev"),
            SelectItem {
                id: 1,
                label: Cow::Borrowed("Add new"),
                role: SelectItemRole::Action,
                filter_mode: SelectFilterMode::AlwaysVisible,
            },
        ];
        let mut state = SelectState::new();
        state.open_with_selected(&items, Some(&0), 4);

        state.handle_key(key(KeyCode::Char('z')), &items, 4);

        assert_eq!(state.resolve_items(&items).item_index(0), Some(1));
        assert_eq!(state.resolve_items(&items).len(), 1);
    }
}
