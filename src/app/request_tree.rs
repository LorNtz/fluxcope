use crate::capture::{CaptureSequence, CaptureSummary};
use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    sync::Arc,
};
use url::Url;

const ORIGIN_IDENTIFIER_PREFIX: &str = "origin:";
const SEGMENT_IDENTIFIER_PREFIX: &str = "segment:";
const REQUEST_IDENTIFIER_PREFIX: &str = "request:";

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct RequestNodeId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RequestNodeKind {
    Origin,
    Path,
    Capture(CaptureSequence),
}

#[derive(Clone, Debug)]
struct RequestNode {
    identifier: Arc<str>,
    label: Arc<str>,
    kind: RequestNodeKind,
    parent: Option<RequestNodeId>,
    children: Vec<RequestNodeId>,
    branch_children: usize,
    leaf_count: usize,
}

#[derive(Clone, Debug)]
enum PathHashEntry {
    One(RequestNodeId),
    Collision(Vec<RequestNodeId>),
}

impl PathHashEntry {
    fn push(&mut self, id: RequestNodeId) {
        match self {
            Self::One(first) => *self = Self::Collision(vec![*first, id]),
            Self::Collision(ids) => ids.push(id),
        }
    }

    fn iter(&self) -> impl Iterator<Item = RequestNodeId> + '_ {
        let (first, rest) = match self {
            Self::One(id) => (Some(*id), &[][..]),
            Self::Collision(ids) => (None, ids.as_slice()),
        };
        first.into_iter().chain(rest.iter().copied())
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct RequestTreeModel {
    roots: Vec<RequestNodeId>,
    nodes: Vec<RequestNode>,
    logical_positions: Vec<usize>,
    nodes_by_path_hash: HashMap<u64, PathHashEntry>,
}

#[derive(Clone, Debug)]
pub(crate) struct RequestTreeNodeSnapshot {
    pub identifier: String,
    pub label: String,
    pub leaf_count: usize,
    pub branch: bool,
    pub children: Vec<RequestTreeNodeSnapshot>,
}

impl RequestTreeModel {
    pub(crate) fn from_requests(requests: impl IntoIterator<Item = CaptureSummary>) -> Self {
        let mut model = Self::default();
        let mut interner = HashMap::<String, Arc<str>>::new();
        let mut branches = HashMap::<(Option<RequestNodeId>, Arc<str>), RequestNodeId>::new();
        for capture in requests {
            let entry = RequestTreeEntry::from(&capture);
            let origin = intern(&mut interner, entry.origin);
            let mut parent = model.branch_mut_or_insert(
                None,
                Arc::clone(&origin),
                RequestNodeKind::Origin,
                &mut branches,
            );
            model.nodes[parent.0 as usize].leaf_count += 1;
            for segment in parent_segments(&entry.segments) {
                let segment = intern(&mut interner, segment.clone());
                parent = model.branch_mut_or_insert(
                    Some(parent),
                    segment,
                    RequestNodeKind::Path,
                    &mut branches,
                );
                model.nodes[parent.0 as usize].leaf_count += 1;
            }
            let label = intern(
                &mut interner,
                entry
                    .segments
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "/".to_string()),
            );
            model.insert_capture(parent, label, capture.sequence);
        }
        model.rebuild_search_index();
        model
    }

    fn branch_mut_or_insert(
        &mut self,
        parent: Option<RequestNodeId>,
        label: Arc<str>,
        kind: RequestNodeKind,
        branches: &mut HashMap<(Option<RequestNodeId>, Arc<str>), RequestNodeId>,
    ) -> RequestNodeId {
        if let Some(node) = branches.get(&(parent, Arc::clone(&label))) {
            return *node;
        }
        let identifier: Arc<str> = match kind {
            RequestNodeKind::Origin => origin_identifier(&label).into(),
            RequestNodeKind::Path => segment_identifier(&label).into(),
            RequestNodeKind::Capture(_) => unreachable!(),
        };
        let id = self.push_node(RequestNode {
            identifier,
            label: Arc::clone(&label),
            kind,
            parent,
            children: Vec::new(),
            branch_children: 0,
            leaf_count: 0,
        });
        branches.insert((parent, label), id);
        match parent {
            Some(parent) => {
                let node = &mut self.nodes[parent.0 as usize];
                node.children.insert(node.branch_children, id);
                node.branch_children += 1;
            }
            None => self.roots.push(id),
        }
        id
    }

    fn insert_capture(
        &mut self,
        parent: RequestNodeId,
        label: Arc<str>,
        sequence: CaptureSequence,
    ) {
        let id = self.push_node(RequestNode {
            identifier: request_identifier(sequence).into(),
            label,
            kind: RequestNodeKind::Capture(sequence),
            parent: Some(parent),
            children: Vec::new(),
            branch_children: 0,
            leaf_count: 1,
        });
        self.nodes[parent.0 as usize].children.push(id);
    }

    fn push_node(&mut self, node: RequestNode) -> RequestNodeId {
        let id = RequestNodeId(
            self.nodes
                .len()
                .try_into()
                .expect("request tree node limit"),
        );
        self.nodes.push(node);
        id
    }

    pub(in crate::app) fn visible_paths(&self, opened_paths: &[Vec<String>]) -> Vec<Vec<String>> {
        let opened_paths = opened_paths.iter().cloned().collect::<HashSet<_>>();
        let mut visible = Vec::new();
        let mut path = Vec::new();
        self.collect_visible(&self.roots, &opened_paths, &mut path, &mut visible);
        visible
    }

    pub(crate) fn contains(&self, path: &[String]) -> bool {
        self.logical_position(path).is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(crate) fn logical_position(&self, path: &[String]) -> Option<usize> {
        let hash = hash_path(path.iter().map(String::as_str));
        self.nodes_by_path_hash.get(&hash)?.iter().find_map(|id| {
            self.node_matches_path(id, path)
                .then_some(self.logical_positions[id.0 as usize])
        })
    }

    pub(crate) fn visit_logical_nodes(
        &self,
        mut visitor: impl FnMut(usize, &Arc<str>, &[Arc<str>]) -> ControlFlow<()>,
    ) -> bool {
        let mut path = Vec::new();
        Self::visit_nodes(
            &self.nodes,
            &self.logical_positions,
            &self.roots,
            &mut path,
            &mut visitor,
        )
        .is_continue()
    }

    pub(in crate::app) fn sequences_under(&self, path: &[String]) -> Vec<CaptureSequence> {
        let Some(root) = self.find(path) else {
            return Vec::new();
        };
        let mut sequences = Vec::new();
        self.collect_sequences(root, &mut sequences);
        sequences
    }

    pub(in crate::app) fn branch_paths(&self) -> Vec<Vec<String>> {
        let mut paths = Vec::new();
        let mut path = Vec::new();
        self.collect_branch_paths(&self.roots, &mut path, &mut paths);
        paths
    }

    pub(crate) fn snapshot(&self) -> Vec<RequestTreeNodeSnapshot> {
        self.roots
            .iter()
            .map(|id| self.snapshot_node(*id))
            .collect()
    }

    fn find(&self, path: &[String]) -> Option<RequestNodeId> {
        let mut siblings = self.roots.as_slice();
        let mut found = None;
        for identifier in path {
            let id = siblings
                .iter()
                .copied()
                .find(|id| self.nodes[id.0 as usize].identifier.as_ref() == identifier)?;
            found = Some(id);
            siblings = &self.nodes[id.0 as usize].children;
        }
        found
    }

    fn neighboring_paths(&self, path: &[String]) -> (Option<Vec<String>>, Option<Vec<String>>) {
        let Some(id) = self.find(path) else {
            return (None, None);
        };
        let node = &self.nodes[id.0 as usize];
        let siblings = node
            .parent
            .map(|parent| self.nodes[parent.0 as usize].children.as_slice())
            .unwrap_or(self.roots.as_slice());
        let Some(index) = siblings.iter().position(|sibling| *sibling == id) else {
            return (None, None);
        };
        let previous = index.checked_sub(1).and_then(|index| siblings.get(index));
        let next = siblings.get(index + 1);
        (
            previous.map(|id| self.path(*id)),
            next.map(|id| self.path(*id)),
        )
    }

    fn path(&self, mut id: RequestNodeId) -> Vec<String> {
        let mut reversed = Vec::new();
        loop {
            let node = &self.nodes[id.0 as usize];
            reversed.push(node.identifier.to_string());
            let Some(parent) = node.parent else { break };
            id = parent;
        }
        reversed.reverse();
        reversed
    }

    fn collect_visible(
        &self,
        nodes: &[RequestNodeId],
        opened: &HashSet<Vec<String>>,
        path: &mut Vec<String>,
        output: &mut Vec<Vec<String>>,
    ) {
        for id in nodes {
            let node = &self.nodes[id.0 as usize];
            path.push(node.identifier.to_string());
            output.push(path.clone());
            if opened.contains(path) {
                self.collect_visible(&node.children, opened, path, output);
            }
            path.pop();
        }
    }

    fn collect_branch_paths(
        &self,
        nodes: &[RequestNodeId],
        path: &mut Vec<String>,
        output: &mut Vec<Vec<String>>,
    ) {
        for id in nodes {
            let node = &self.nodes[id.0 as usize];
            path.push(node.identifier.to_string());
            if !matches!(node.kind, RequestNodeKind::Capture(_)) {
                output.push(path.clone());
                self.collect_branch_paths(&node.children, path, output);
            }
            path.pop();
        }
    }

    fn collect_sequences(&self, id: RequestNodeId, output: &mut Vec<CaptureSequence>) {
        let node = &self.nodes[id.0 as usize];
        if let RequestNodeKind::Capture(sequence) = node.kind {
            output.push(sequence);
        } else {
            for child in &node.children {
                self.collect_sequences(*child, output);
            }
        }
    }

    fn snapshot_node(&self, id: RequestNodeId) -> RequestTreeNodeSnapshot {
        let node = &self.nodes[id.0 as usize];
        RequestTreeNodeSnapshot {
            identifier: node.identifier.to_string(),
            label: node.label.to_string(),
            leaf_count: node.leaf_count,
            branch: !matches!(node.kind, RequestNodeKind::Capture(_)),
            children: node
                .children
                .iter()
                .map(|child| self.snapshot_node(*child))
                .collect(),
        }
    }

    fn rebuild_search_index(&mut self) {
        let mut logical_positions = vec![0; self.nodes.len()];
        let mut nodes_by_path_hash = HashMap::<u64, PathHashEntry>::new();
        let mut next_position = 0_usize;
        Self::index_nodes(
            &self.nodes,
            &self.roots,
            PATH_HASH_OFFSET,
            &mut next_position,
            &mut logical_positions,
            &mut nodes_by_path_hash,
        );
        self.logical_positions = logical_positions;
        self.nodes_by_path_hash = nodes_by_path_hash;
    }

    fn index_nodes(
        nodes: &[RequestNode],
        ids: &[RequestNodeId],
        parent_hash: u64,
        next_position: &mut usize,
        logical_positions: &mut [usize],
        nodes_by_path_hash: &mut HashMap<u64, PathHashEntry>,
    ) {
        for id in ids {
            let node = &nodes[id.0 as usize];
            let path_hash = extend_path_hash(parent_hash, &node.identifier);
            logical_positions[id.0 as usize] = *next_position;
            *next_position = next_position.saturating_add(1);
            nodes_by_path_hash
                .entry(path_hash)
                .and_modify(|entry| entry.push(*id))
                .or_insert(PathHashEntry::One(*id));
            Self::index_nodes(
                nodes,
                &node.children,
                path_hash,
                next_position,
                logical_positions,
                nodes_by_path_hash,
            );
        }
    }

    fn visit_nodes(
        nodes: &[RequestNode],
        logical_positions: &[usize],
        ids: &[RequestNodeId],
        path: &mut Vec<Arc<str>>,
        visitor: &mut impl FnMut(usize, &Arc<str>, &[Arc<str>]) -> ControlFlow<()>,
    ) -> ControlFlow<()> {
        for id in ids {
            let node = &nodes[id.0 as usize];
            path.push(Arc::clone(&node.identifier));
            visitor(logical_positions[id.0 as usize], &node.label, path)?;
            Self::visit_nodes(nodes, logical_positions, &node.children, path, visitor)?;
            path.pop();
        }
        ControlFlow::Continue(())
    }

    fn node_matches_path(&self, mut id: RequestNodeId, path: &[String]) -> bool {
        let mut identifiers = path.iter().rev();
        loop {
            let node = &self.nodes[id.0 as usize];
            if identifiers.next().map(String::as_str) != Some(node.identifier.as_ref()) {
                return false;
            }
            let Some(parent) = node.parent else {
                return identifiers.next().is_none();
            };
            id = parent;
        }
    }
}

const PATH_HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const PATH_HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

fn hash_path<'a>(identifiers: impl IntoIterator<Item = &'a str>) -> u64 {
    identifiers
        .into_iter()
        .fold(PATH_HASH_OFFSET, extend_path_hash)
}

fn extend_path_hash(mut hash: u64, identifier: &str) -> u64 {
    for byte in identifier
        .len()
        .to_le_bytes()
        .iter()
        .chain(identifier.as_bytes())
    {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PATH_HASH_PRIME);
    }
    hash
}

fn intern(interner: &mut HashMap<String, Arc<str>>, value: String) -> Arc<str> {
    if let Some(interned) = interner.get(&value) {
        return Arc::clone(interned);
    }
    let interned: Arc<str> = value.clone().into();
    interner.insert(value, Arc::clone(&interned));
    interned
}

pub(in crate::app) struct DeleteSelectionContext<'a> {
    deleted_path: &'a [String],
    tree_before_delete: &'a RequestTreeModel,
    tree_after_delete: &'a RequestTreeModel,
    visible_paths: &'a [Vec<String>],
}

impl<'a> DeleteSelectionContext<'a> {
    /// Creates a chooser for the next selected row after a delete operation.
    pub(in crate::app) fn new(
        deleted_path: &'a [String],
        tree_before_delete: &'a RequestTreeModel,
        tree_after_delete: &'a RequestTreeModel,
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
    pub(in crate::app) fn selected_path(&self) -> Vec<String> {
        if let Some(deleted_start_path) = self.first_removed_prefix() {
            let (previous_sibling, next_sibling) = self
                .tree_before_delete
                .neighboring_paths(&deleted_start_path);

            if let Some(previous_sibling) =
                previous_sibling.filter(|path| self.tree_after_delete.contains(path))
                && let Some(visible_descendant) = self.deepest_visible_descendant(&previous_sibling)
            {
                return visible_descendant;
            }

            if let Some(next_sibling) =
                next_sibling.filter(|path| self.tree_after_delete.contains(path))
                && self.visible_paths.contains(&next_sibling)
            {
                return next_sibling;
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
pub(in crate::app) struct RequestTreeEntry {
    pub(in crate::app) origin: String,
    segments: Vec<String>,
    request_sequence: CaptureSequence,
}

impl RequestTreeEntry {
    fn request_identifier(&self) -> String {
        request_identifier(self.request_sequence)
    }

    pub(in crate::app) fn request_path(&self) -> Vec<String> {
        let mut path = self.parent_path();
        path.push(self.request_identifier());
        path
    }

    pub(in crate::app) fn branch_paths(&self) -> Vec<Vec<String>> {
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

impl From<&CaptureSummary> for RequestTreeEntry {
    fn from(capture: &CaptureSummary) -> Self {
        let request = &capture.request;
        let display_uri = request.display_uri();
        let (origin, segments) = Url::parse(display_uri)
            .ok()
            .filter(Url::has_host)
            .map(|url| {
                (
                    absolute_url_origin(&url),
                    path_segments_with_query(url.path(), url.query()),
                )
            })
            .unwrap_or_else(|| {
                let origin = request_header_slice(&request.headers, "host")
                    .map(|host| format!("https://{host}"))
                    .unwrap_or_else(|| "(unknown host)".to_string());
                let (path, query) = display_uri
                    .split_once('?')
                    .map_or((display_uri, None), |(path, query)| (path, Some(query)));
                (origin, path_segments_with_query(path, query))
            });

        Self {
            origin,
            segments,
            request_sequence: capture.sequence,
        }
    }
}

pub(in crate::app) fn selected_request_sequence(path: &[String]) -> Option<CaptureSequence> {
    path.last()?
        .strip_prefix(REQUEST_IDENTIFIER_PREFIX)?
        .parse()
        .ok()
        .map(CaptureSequence::new)
}

pub(in crate::app) fn origin_identifier(origin: &str) -> String {
    format!("{ORIGIN_IDENTIFIER_PREFIX}{origin}")
}

fn segment_identifier(segment: &str) -> String {
    format!("{SEGMENT_IDENTIFIER_PREFIX}{segment}")
}

fn request_identifier(sequence: CaptureSequence) -> String {
    format!("{REQUEST_IDENTIFIER_PREFIX}{}", sequence.value())
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

fn request_header_slice<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
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
    use crate::capture::{CaptureRecord, CapturedExchange};
    use http::Method;

    fn captured(sequence: u64, uri: &str) -> CapturedExchange {
        CapturedExchange {
            sequence: CaptureSequence::new(sequence),
            method: Method::GET,
            uri: uri.to_string(),
            mapped_uri: None,
            local_path: None,
            status: None,
            req_headers: vec![("host".to_string(), "fallback.example.com".to_string())],
            res_headers: Vec::new(),
            req_body: None,
            res_body: None,
        }
    }

    fn entry(capture: CapturedExchange) -> RequestTreeEntry {
        let record = CaptureRecord::from_completed(capture);
        RequestTreeEntry::from(&record.summary())
    }

    #[test]
    fn capture_summary_splits_absolute_display_url() {
        let entry = entry(captured(
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
    fn capture_summary_uses_effective_mapped_url() {
        let mut capture = captured(7, "https://a.com/original/path?a=1");
        capture.mapped_uri = Some("http://b.test.com/mapped/path?a=1".to_string());
        let entry = entry(capture);

        assert_eq!(entry.origin, "http://b.test.com");
        assert_eq!(entry.segments, ["mapped", "path?a=1"]);
        assert_eq!(
            entry.request_path(),
            ["origin:http://b.test.com", "segment:mapped", "request:7"]
        );
    }

    #[test]
    fn capture_summary_uses_host_header_for_origin_form_uri() {
        let entry = entry(captured(0, "/common/getSomeOtherInfo"));

        assert_eq!(entry.origin, "https://fallback.example.com");
        assert_eq!(entry.segments, ["common", "getSomeOtherInfo"]);
    }
}
