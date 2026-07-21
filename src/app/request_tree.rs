#[cfg(test)]
use crate::capture::CapturedExchange;
use crate::capture::{CaptureSequence, CaptureSummary};
use std::{
    collections::{HashMap, HashSet},
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

#[derive(Clone, Debug, Default)]
pub(in crate::app) struct RequestTreeModel {
    roots: Vec<RequestNodeId>,
    nodes: Vec<RequestNode>,
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
    pub(in crate::app) fn from_requests(
        requests: impl IntoIterator<Item = CaptureSummary>,
    ) -> Self {
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

    pub(in crate::app) fn contains(&self, path: &[String]) -> bool {
        self.find(path).is_some()
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
pub struct RequestTreeEntry {
    pub origin: String,
    pub segments: Vec<String>,
    request_sequence: CaptureSequence,
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

#[cfg(test)]
impl From<&CapturedExchange> for RequestTreeEntry {
    fn from(req: &CapturedExchange) -> Self {
        let display_uri = request_display_uri(req);
        let (origin, segments) = Url::parse(display_uri)
            .ok()
            .filter(Url::has_host)
            .map(|url| {
                (
                    absolute_url_origin(&url),
                    path_segments_with_query(url.path(), url.query()),
                )
            })
            .unwrap_or_else(|| fallback_origin_and_segments(req));

        Self {
            origin,
            segments,
            request_sequence: req.sequence,
        }
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

#[cfg(test)]
fn request_display_uri(req: &CapturedExchange) -> &str {
    req.display_uri()
}

#[cfg(test)]
fn fallback_origin_and_segments(req: &CapturedExchange) -> (String, Vec<String>) {
    let origin = request_header(req, "host")
        .map(|host| format!("https://{host}"))
        .unwrap_or_else(|| "(unknown host)".to_string());
    let display_uri = request_display_uri(req);
    let (path, query) = display_uri
        .split_once('?')
        .map_or((display_uri, None), |(path, query)| (path, Some(query)));

    (origin, path_segments_with_query(path, query))
}

#[cfg(test)]
fn request_header<'a>(req: &'a CapturedExchange, name: &str) -> Option<&'a str> {
    request_header_slice(&req.req_headers, name)
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
