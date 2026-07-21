use crate::capture::{CaptureSequence, CapturedExchange};
use std::collections::HashSet;
use url::Url;

const ORIGIN_IDENTIFIER_PREFIX: &str = "origin:";
const SEGMENT_IDENTIFIER_PREFIX: &str = "segment:";
const REQUEST_IDENTIFIER_PREFIX: &str = "request:";

#[derive(Debug, Default)]
pub(in crate::app) struct RequestPathTree {
    roots: Vec<RequestTreePathNode>,
}

impl RequestPathTree {
    /// Builds an internal path-only tree from captured requests.
    ///
    /// This tree mirrors the UI tree's identifier structure without depending on UI
    /// rendering code, which lets app-state logic make selection decisions directly.
    pub(in crate::app) fn from_requests<'a>(
        requests: impl IntoIterator<Item = &'a CapturedExchange>,
    ) -> Self {
        let mut tree = Self::default();

        for req in requests {
            tree.insert_request_path(RequestTreeEntry::from(req).request_path());
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
    pub(in crate::app) fn visible_paths(&self, opened_paths: &[Vec<String>]) -> Vec<Vec<String>> {
        let opened_paths = opened_paths.iter().cloned().collect::<HashSet<_>>();
        let mut visible_paths = Vec::new();
        RequestTreePathNode::collect_visible_paths(&self.roots, &opened_paths, &mut visible_paths);
        visible_paths
    }

    /// Returns true when the exact tree path exists in the internal tree.
    pub(in crate::app) fn contains(&self, path: &[String]) -> bool {
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

pub(in crate::app) struct DeleteSelectionContext<'a> {
    deleted_path: &'a [String],
    tree_before_delete: &'a RequestPathTree,
    tree_after_delete: &'a RequestPathTree,
    visible_paths: &'a [Vec<String>],
}

impl<'a> DeleteSelectionContext<'a> {
    /// Creates a chooser for the next selected row after a delete operation.
    pub(in crate::app) fn new(
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

fn request_display_uri(req: &CapturedExchange) -> &str {
    req.display_uri()
}

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

fn request_header<'a>(req: &'a CapturedExchange, name: &str) -> Option<&'a str> {
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
