//! Embedded languages for incremental highlighting.
//!
//! `tree_sitter_highlight` highlights a language with injections as a stack
//! of layers: the host tree plus a tree per injected range (the CSS of an
//! HTML `<style>`, the code of a Markdown fence) or per combined injection
//! (all tagged templates of a JavaScript file, parsed as one tree). It builds
//! every layer on every pass. Here the trees are kept across edits in a
//! [`LayerCache`], keyed by language and included ranges, and an edited one
//! is re-parsed incrementally only once it is needed again.
//!
//! [`LayerCache::highlight`] replays `HighlightIter` over a window of the
//! document: the layers' capture streams merged by the same sort key,
//! injections spawning layers as their captures come up, one style stack
//! shared by all layers, the same highlight dropped when a deeper layer has
//! already started it. Each stream only holds the captures that reach into
//! the window, and the window starts early enough that no highlight of any
//! layer crosses its start. At that point the full pass has closed every
//! highlight it opened, so the replay starts from the state the full pass is
//! in there and produces the same styles from then on.

use ratatui::style::Style;
use ropey::Rope;
use std::collections::{HashSet, VecDeque};
use std::ops::Range as Bytes;
use tree_sitter::{
    InputEdit, Node, Parser, Point, Query, QueryCursor, QueryMatch, Range, StreamingIterator, Tree,
};

use crate::document::{parse, QueryHighlights, RopeText};

/// Finds the queries of an embedded language by the name its host gives.
pub(crate) type Resolver<'r> = dyn Fn(&str) -> Option<QueryHighlights<'static>> + 'r;

/// Layers kept across renders; the least recently used go past this.
const MAX_LAYERS: usize = 256;

/// Injections nest no deeper than this. `tree_sitter_highlight` has no such
/// bound, but no grammar comes close; it only stops a self-injection loop.
const MAX_DEPTH: usize = 16;

/// A layer as an injection asks for it.
#[derive(Clone)]
pub(crate) struct Injection {
    highlights: QueryHighlights<'static>,
    depth: usize,
    ranges: Vec<Range>,
}

impl Injection {
    fn edit(&mut self, edit: &InputEdit) {
        self.ranges.iter_mut().for_each(|r| edit_range(r, edit));
    }
}

/// The combined injections of the host tree, kept across edits: matching
/// them means a query over the whole tree, so they are worked out again
/// only when an edit may have changed them.
pub(crate) struct HostCombined {
    injections: Vec<Injection>,
    /// Per match, the smallest node holding all its captures. A match can
    /// only go away through a change inside its node.
    matches: Vec<Bytes<usize>>,
}

impl HostCombined {
    pub fn injections(&self) -> &[Injection] {
        &self.injections
    }

    pub fn edit(&mut self, edit: &InputEdit) {
        self.injections.iter_mut().for_each(|i| i.edit(edit));
        self.matches.iter_mut().for_each(|m| edit_bytes(m, edit));
    }
}

/// The host tree, which the document keeps itself.
pub(crate) struct HostLayer<'a> {
    pub tree: &'a Tree,
    pub highlights: QueryHighlights<'static>,
    /// The host's combined injections (see
    /// [`LayerCache::host_combined_injections`]).
    pub combined: &'a [Injection],
}

struct Layer {
    highlights: QueryHighlights<'static>,
    depth: usize,
    /// The included ranges, in the current text's coordinates.
    ranges: Vec<Range>,
    tree: Tree,
    /// The layer's own combined injections; empty while `dirty`.
    combined: Vec<Injection>,
    /// An edit touched the layer since `tree` was parsed.
    dirty: bool,
    /// The last [`LayerCache::highlight`] that used the layer.
    used: u64,
}

/// The trees of the languages embedded in a document.
pub(crate) struct LayerCache {
    parser: Parser,
    cursor: QueryCursor,
    layers: Vec<Layer>,
    generation: u64,
}

impl LayerCache {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            cursor: QueryCursor::new(),
            layers: Vec::new(),
            generation: 0,
        }
    }

    pub fn clear(&mut self) {
        self.layers.clear();
    }

    /// Follow an edit of the document: shift every layer past it, and mark
    /// the layers it touched for re-parsing.
    pub fn edit(&mut self, edit: &InputEdit) {
        for layer in &mut self.layers {
            let first = layer.ranges.first().map_or(0, |r| r.start_byte);
            let last = layer.ranges.last().map_or(0, |r| r.end_byte);
            if edit.start_byte <= last && edit.old_end_byte >= first {
                layer.dirty = true;
                layer.combined.clear();
            } else {
                layer.combined.iter_mut().for_each(|i| i.edit(edit));
            }
            layer.tree.edit(edit);
            layer.ranges.iter_mut().for_each(|r| edit_range(r, edit));
        }
    }

    /// Drop the least recently used layers past [`MAX_LAYERS`]. Lines
    /// already highlighted do not need them; a stale line brings back what
    /// it needs.
    pub fn evict(&mut self) {
        if self.layers.len() > MAX_LAYERS {
            self.layers
                .sort_by_key(|layer| std::cmp::Reverse(layer.used));
            self.layers.truncate(MAX_LAYERS);
        }
    }

    /// The combined injections of the host tree. These are matched over the
    /// whole tree, as `tree_sitter_highlight` does: every match of a pattern
    /// adds its ranges to one layer, whose language the last match names.
    pub fn host_combined_injections(
        &mut self,
        tree: &Tree,
        highlights: &QueryHighlights<'static>,
        rope: &Rope,
        resolve: &Resolver,
    ) -> HostCombined {
        let (injections, matches) = combined_injections(
            &mut self.cursor,
            tree,
            highlights,
            0,
            &[whole_document()],
            None,
            rope,
            resolve,
        );
        HostCombined {
            injections,
            matches,
        }
    }

    /// Whether `combined`, edited along, may no longer be the combined
    /// injections of the re-parsed `tree`: `regions` (what the edits wrote
    /// and where the syntax changed) reach a match it was made of, or hold
    /// a match now.
    pub fn host_combined_changed(
        &mut self,
        combined: &HostCombined,
        tree: &Tree,
        highlights: &QueryHighlights,
        regions: &[Bytes<usize>],
        rope: &Rope,
    ) -> bool {
        let Some(query) = highlights.combined_injections else {
            return false;
        };
        let touches = |a: &Bytes<usize>, b: &Bytes<usize>| a.start <= b.end && b.start <= a.end;
        if regions
            .iter()
            .any(|region| combined.matches.iter().any(|m| touches(m, region)))
        {
            return true;
        }
        regions.iter().any(|region| {
            self.cursor
                .set_byte_range(region.start.saturating_sub(1)..region.end + 1);
            self.cursor
                .matches(query, tree.root_node(), RopeText(rope))
                .next()
                .is_some()
        })
    }

    /// Styled runs covering at least `window`, as `tree_sitter_highlight`
    /// would colour those bytes in a pass over the whole document.
    pub fn highlight(
        &mut self,
        host: &HostLayer,
        rope: &Rope,
        window: Bytes<usize>,
        resolve: &Resolver,
        styles: &dyn Fn(usize) -> Style,
        default_style: Style,
    ) -> Vec<(Bytes<usize>, Style)> {
        self.generation += 1;
        let mut start = window.start;
        loop {
            let mut gather = Gather {
                cache: self,
                host,
                rope,
                resolve,
                range: start..window.end,
                streams: Vec::new(),
                spawns: Vec::new(),
            };
            let group = gather.host_group();
            let Gather {
                streams, spawns, ..
            } = gather;
            // One tree's highlights nest, so they need no earlier start; it
            // is highlights of different layers that can overlap.
            if streams.len() > 1 {
                let earliest = streams
                    .iter()
                    .flat_map(|s| s.caps.iter().map(move |c| (s, c)))
                    .filter(|(s, c)| c.spawn.is_none() && s.highlight(c).is_some())
                    .map(|(_, c)| c.start)
                    .min();
                if let Some(earliest) = earliest.filter(|&e| e < start) {
                    start = earliest;
                    continue;
                }
            }
            let mut replay = Replay {
                streams,
                spawns,
                active: Vec::new(),
                pos: start,
                stack: Vec::new(),
                runs: Vec::new(),
                last_highlight: None,
                styles,
                default_style,
            };
            return replay.run(group, start..window.end);
        }
    }

    /// The cached layer for `request`, parsed if need be. `None` when the
    /// ranges are not valid included ranges, which `tree_sitter_highlight`
    /// skips the same way.
    fn layer_for(
        &mut self,
        request: &Injection,
        parent: Option<&str>,
        rope: &Rope,
        resolve: &Resolver,
    ) -> Option<usize> {
        let generation = self.generation;
        let same = |layer: &Layer| {
            layer.highlights.name == request.highlights.name && layer.depth == request.depth
        };
        let exact = self
            .layers
            .iter()
            .position(|layer| same(layer) && layer.ranges == request.ranges);
        if let Some(i) = exact.filter(|&i| !self.layers[i].dirty) {
            self.layers[i].used = generation;
            return Some(i);
        }
        // A layer whose text was edited, or whose injection grew or shrank:
        // re-parse it reusing what is left of its old tree.
        let reusable = exact.or_else(|| {
            self.layers.iter().position(|layer| {
                same(layer) && layer.used != generation && overlaps(&layer.ranges, &request.ranges)
            })
        });
        let old_tree = reusable.map(|i| &self.layers[i].tree);
        self.parser.set_included_ranges(&request.ranges).ok()?;
        let tree = parse(
            &mut self.parser,
            request.highlights.language,
            rope,
            old_tree,
        )?;
        let (combined, _) = combined_injections(
            &mut self.cursor,
            &tree,
            &request.highlights,
            request.depth,
            &request.ranges,
            parent,
            rope,
            resolve,
        );
        let layer = Layer {
            highlights: request.highlights,
            depth: request.depth,
            ranges: request.ranges.clone(),
            tree,
            combined,
            dirty: false,
            used: generation,
        };
        Some(match reusable {
            Some(i) => {
                self.layers[i] = layer;
                i
            }
            None => {
                self.layers.push(layer);
                self.layers.len() - 1
            }
        })
    }

    #[cfg(test)]
    pub fn trees(&self) -> impl Iterator<Item = (&str, &[Range], &Tree)> {
        self.layers
            .iter()
            .filter(|l| !l.dirty)
            .map(|l| (l.highlights.name, l.ranges.as_slice(), &l.tree))
    }
}

/// One capture of a layer's highlights query.
#[derive(Clone, Copy)]
struct Cap {
    start: usize,
    end: usize,
    node: usize,
    match_id: u32,
    index: u32,
    /// For a capture of an injection pattern, the [`Spawn`] of its match.
    spawn: Option<usize>,
}

/// A layer's captures that reach into the window, in query order.
struct Stream {
    highlights: QueryHighlights<'static>,
    depth: usize,
    caps: Vec<Cap>,
    next: usize,
    /// Where the highlights this layer has started end, innermost last.
    ends: Vec<usize>,
    /// Matches dropped from the stream (`QueryMatch::remove`).
    removed: HashSet<u32>,
}

impl Stream {
    fn highlight(&self, cap: &Cap) -> Option<usize> {
        self.highlights
            .capture_highlights
            .get(cap.index as usize)
            .copied()
            .flatten()
    }
}

/// The layers an injection match brings in when its first capture comes up.
struct Spawn {
    /// The stream the match belongs to.
    owner: usize,
    match_id: u32,
    /// The injected layer then the combined injections under it, as
    /// `HighlightIterLayer::new` returns them.
    group: Vec<usize>,
}

/// Collects the capture streams of every layer that reaches into `range`.
struct Gather<'g, 'h> {
    cache: &'g mut LayerCache,
    host: &'g HostLayer<'h>,
    rope: &'g Rope,
    resolve: &'g Resolver<'g>,
    range: Bytes<usize>,
    streams: Vec<Stream>,
    spawns: Vec<Spawn>,
}

#[derive(Clone, Copy)]
enum LayerTree {
    Host,
    Cached(usize),
}

impl Gather<'_, '_> {
    /// The host stream then those of its combined injections.
    fn host_group(&mut self) -> Vec<usize> {
        let mut group = vec![self.stream(LayerTree::Host, self.host.highlights, 0)];
        self.add_layers(self.host.combined.to_vec(), None, &mut group);
        group
    }

    /// Bring in the layers `requests` ask for and, breadth first, the
    /// combined injections under them.
    fn add_layers(
        &mut self,
        requests: Vec<Injection>,
        parent: Option<&str>,
        group: &mut Vec<usize>,
    ) {
        let mut queue = VecDeque::from(requests);
        while let Some(request) = queue.pop_front() {
            if request.depth > MAX_DEPTH {
                continue;
            }
            let Some(i) = self
                .cache
                .layer_for(&request, parent, self.rope, self.resolve)
            else {
                continue;
            };
            queue.extend(self.cache.layers[i].combined.iter().cloned());
            group.push(self.stream(LayerTree::Cached(i), request.highlights, request.depth));
        }
    }

    fn stream(
        &mut self,
        layer: LayerTree,
        highlights: QueryHighlights<'static>,
        depth: usize,
    ) -> usize {
        let rope = self.rope;
        let resolve = self.resolve;
        let top = self.host.highlights.name;
        let LayerCache { layers, cursor, .. } = &mut *self.cache;
        let (tree, ranges) = match layer {
            LayerTree::Host => (self.host.tree, vec![whole_document()]),
            LayerTree::Cached(i) => (&layers[i].tree, layers[i].ranges.clone()),
        };

        let mut caps = Vec::new();
        // Per injection match: its id and the layer it asks for. An injected
        // layer lies within its content node, so when it reaches into the
        // window the content's capture is in the stream to bring it in.
        let mut injections: Vec<(u32, Option<Injection>)> = Vec::new();
        cursor.set_byte_range(self.range.clone());
        let mut captures = cursor.captures(highlights.query, tree.root_node(), RopeText(rope));
        while let Some((query_match, capture_index)) = captures.next() {
            let capture = query_match.captures()[*capture_index];
            let spawn = (query_match.pattern_index < highlights.injection_patterns).then(|| {
                let id = query_match.id();
                injections
                    .iter()
                    .position(|(match_id, _)| *match_id == id)
                    .unwrap_or_else(|| {
                        let request = injection_request(
                            &highlights,
                            Some(top),
                            query_match,
                            &ranges,
                            depth,
                            rope,
                            resolve,
                        );
                        injections.push((id, request));
                        injections.len() - 1
                    })
            });
            caps.push(Cap {
                start: capture.node.start_byte(),
                end: capture.node.end_byte(),
                node: capture.node.id(),
                match_id: query_match.id(),
                index: capture.index,
                spawn,
            });
        }

        let me = self.streams.len();
        let base = self.spawns.len();
        caps.iter_mut()
            .filter_map(|cap| cap.spawn.as_mut())
            .for_each(|spawn| *spawn += base);
        self.streams.push(Stream {
            highlights,
            depth,
            caps,
            next: 0,
            ends: Vec::new(),
            removed: HashSet::new(),
        });
        for (match_id, _) in &injections {
            self.spawns.push(Spawn {
                owner: me,
                match_id: *match_id,
                group: Vec::new(),
            });
        }
        for (k, (_, request)) in injections.into_iter().enumerate() {
            if let Some(request) = request {
                let mut group = Vec::new();
                self.add_layers(vec![request], Some(top), &mut group);
                self.spawns[base + k].group = group;
            }
        }
        me
    }
}

/// The layer an injection match asks for, if its language is known and its
/// ranges are not empty.
fn injection_request(
    highlights: &QueryHighlights<'static>,
    parent: Option<&str>,
    query_match: &QueryMatch,
    parent_ranges: &[Range],
    depth: usize,
    rope: &Rope,
    resolve: &Resolver,
) -> Option<Injection> {
    let (name, content, include_children) =
        injection_for_match(highlights, parent, highlights.query, query_match, rope);
    let next = resolve(&name?)?;
    let ranges = intersect_ranges(parent_ranges, &[content?], include_children);
    (!ranges.is_empty()).then_some(Injection {
        highlights: next,
        depth: depth + 1,
        ranges,
    })
}

enum Event {
    Start(Style),
    End,
}

/// `HighlightIter` over gathered streams, collecting styled runs.
struct Replay<'s> {
    streams: Vec<Stream>,
    spawns: Vec<Spawn>,
    /// Indices into `streams`, ordered by their next boundary.
    active: Vec<usize>,
    pos: usize,
    stack: Vec<Style>,
    runs: Vec<(Bytes<usize>, Style)>,
    last_highlight: Option<(usize, usize, usize)>,
    styles: &'s dyn Fn(usize) -> Style,
    default_style: Style,
}

impl Replay<'_> {
    fn run(&mut self, group: Vec<usize>, window: Bytes<usize>) -> Vec<(Bytes<usize>, Style)> {
        self.active = group;
        self.sort_layers();

        while let Some(&layer) = self.active.first() {
            match self.key(layer) {
                Some((offset, ..)) if offset < window.end => {}
                _ => break,
            }
            let next = self.peek(layer);
            let end = self.streams[layer].ends.last().copied();
            if let Some(end) = end.filter(|&end| next.is_none_or(|cap| end <= cap.start)) {
                self.streams[layer].ends.pop();
                self.emit(end, Event::End);
                continue;
            }
            let Some(cap) = next else { break };
            self.streams[layer].next += 1;

            if let Some(k) = cap.spawn {
                self.spawn(k);
                self.sort_layers();
                continue;
            }

            let depth = self.streams[layer].depth;
            if self.last_highlight.is_some_and(|(start, end, last_depth)| {
                cap.start == start && cap.end == end && depth < last_depth
            }) {
                self.sort_layers();
                continue;
            }

            // Of the captures on one node, the last pattern wins.
            let mut cap = cap;
            while let Some(following) = self.peek(layer).filter(|c| c.node == cap.node) {
                self.streams[layer].next += 1;
                self.streams[layer].removed.insert(cap.match_id);
                cap = following;
            }

            if let Some(highlight) = self.streams[layer].highlight(&cap) {
                self.last_highlight = Some((cap.start, cap.end, depth));
                self.streams[layer].ends.push(cap.end);
                let style = (self.styles)(highlight);
                self.emit(cap.start, Event::Start(style));
                continue;
            }
            self.sort_layers();
        }
        let top = self.top();
        if self.pos < window.end {
            self.runs.push((self.pos..window.end, top));
        }
        std::mem::take(&mut self.runs)
    }

    fn top(&self) -> Style {
        self.stack.last().copied().unwrap_or(self.default_style)
    }

    /// Style the text up to `offset`, then open or close a highlight.
    fn emit(&mut self, offset: usize, event: Event) {
        if self.pos < offset {
            let top = self.top();
            self.runs.push((self.pos..offset, top));
            self.pos = offset;
        }
        match event {
            Event::Start(style) => self.stack.push(style),
            Event::End => {
                self.stack.pop();
            }
        }
        self.sort_layers();
    }

    /// Process an injection match: drop it from its stream and bring in the
    /// layers it asks for.
    fn spawn(&mut self, k: usize) {
        let owner = self.spawns[k].owner;
        let match_id = self.spawns[k].match_id;
        self.streams[owner].removed.insert(match_id);
        for layer in std::mem::take(&mut self.spawns[k].group) {
            self.insert_layer(layer);
        }
    }

    fn peek(&mut self, layer: usize) -> Option<Cap> {
        let stream = &mut self.streams[layer];
        while let Some(cap) = stream.caps.get(stream.next) {
            if !stream.removed.contains(&cap.match_id) {
                return Some(*cap);
            }
            stream.next += 1;
        }
        None
    }

    /// `HighlightIterLayer::sort_key`: the next boundary, ends before
    /// starts, deeper layers first.
    fn key(&mut self, layer: usize) -> Option<(usize, bool, isize)> {
        let depth = -(self.streams[layer].depth as isize);
        let next_start = self.peek(layer).map(|cap| cap.start);
        let next_end = self.streams[layer].ends.last().copied();
        match (next_start, next_end) {
            (Some(start), Some(end)) if start < end => Some((start, true, depth)),
            (Some(_), Some(end)) => Some((end, false, depth)),
            (Some(start), None) => Some((start, true, depth)),
            (None, Some(end)) => Some((end, false, depth)),
            (None, None) => None,
        }
    }

    /// `HighlightIter::sort_layers`.
    fn sort_layers(&mut self) {
        while !self.active.is_empty() {
            if let Some(key) = self.key(self.active[0]) {
                let mut i = 0;
                while i + 1 < self.active.len() {
                    match self.key(self.active[i + 1]) {
                        Some(next) if next < key => i += 1,
                        _ => break,
                    }
                }
                if i > 0 {
                    self.active[0..=i].rotate_left(1);
                }
                break;
            }
            self.active.remove(0);
        }
    }

    /// `HighlightIter::insert_layer`.
    fn insert_layer(&mut self, layer: usize) {
        let Some(key) = self.key(layer) else { return };
        let mut i = 1;
        while i < self.active.len() {
            match self.key(self.active[i]) {
                Some(key_i) if key_i > key => {
                    self.active.insert(i, layer);
                    return;
                }
                Some(_) => i += 1,
                None => {
                    self.active.remove(i);
                }
            }
        }
        self.active.push(layer);
    }
}

/// The combined injections of a layer: per pattern, every match's content
/// in one layer, in the language the last match names
/// (`HighlightIterLayer::new`). Also returns the node around each match.
#[allow(clippy::too_many_arguments)]
fn combined_injections(
    cursor: &mut QueryCursor,
    tree: &Tree,
    highlights: &QueryHighlights<'static>,
    depth: usize,
    ranges: &[Range],
    parent: Option<&str>,
    rope: &Rope,
    resolve: &Resolver,
) -> (Vec<Injection>, Vec<Bytes<usize>>) {
    let Some(query) = highlights.combined_injections else {
        return (Vec::new(), Vec::new());
    };
    let mut by_pattern: Vec<(Option<String>, Vec<Node>, bool)> =
        vec![(None, Vec::new(), false); query.pattern_count()];
    let mut extents = Vec::new();
    cursor.set_byte_range(0..usize::MAX);
    let mut matches = cursor.matches(query, tree.root_node(), RopeText(rope));
    while let Some(query_match) = matches.next() {
        let captures = query_match.captures();
        if let (Some(start), Some(end)) = (
            captures.iter().map(|c| c.node.start_byte()).min(),
            captures.iter().map(|c| c.node.end_byte()).max(),
        ) {
            let node = tree.root_node().descendant_for_byte_range(start, end);
            extents.push(node.map_or(start..end, |n| n.byte_range()));
        }
        let (name, content, include_children) =
            injection_for_match(highlights, parent, query, query_match, rope);
        let entry = &mut by_pattern[query_match.pattern_index];
        if name.is_some() {
            entry.0 = name;
        }
        entry.1.extend(content);
        entry.2 = include_children;
    }
    let injections = by_pattern
        .into_iter()
        .filter_map(|(name, nodes, include_children)| {
            if nodes.is_empty() {
                return None;
            }
            let next = resolve(&name?)?;
            let ranges = intersect_ranges(ranges, &nodes, include_children);
            (!ranges.is_empty()).then_some(Injection {
                highlights: next,
                depth: depth + 1,
                ranges,
            })
        })
        .collect();
    (injections, extents)
}

/// `tree_sitter_highlight::injection_for_match`: the language, the content
/// node and whether the content's children are included.
fn injection_for_match<'tree>(
    highlights: &QueryHighlights,
    parent: Option<&str>,
    query: &Query,
    query_match: &QueryMatch<'_, 'tree>,
    rope: &Rope,
) -> (Option<String>, Option<Node<'tree>>, bool) {
    let mut language = None;
    let mut content = None;
    for capture in query_match.captures() {
        let index = Some(capture.index);
        if index == highlights.language_capture {
            language = node_text(rope, capture.node);
        } else if index == highlights.content_capture {
            content = Some(capture.node);
        }
    }
    let mut include_children = false;
    for property in query.property_settings(query_match.pattern_index) {
        match property.key.as_ref() {
            "injection.language" if language.is_none() => {
                language = property.value.as_deref().map(str::to_string);
            }
            "injection.self" if language.is_none() => {
                language = Some(highlights.name.to_string());
            }
            "injection.parent" if language.is_none() => {
                language = parent.map(str::to_string);
            }
            "injection.include-children" => include_children = true,
            _ => {}
        }
    }
    (language, content, include_children)
}

fn node_text(rope: &Rope, node: Node) -> Option<String> {
    rope.get_byte_slice(node.start_byte()..node.end_byte())
        .map(String::from)
}

fn whole_document() -> Range {
    Range {
        start_byte: 0,
        end_byte: usize::MAX,
        start_point: Point::new(0, 0),
        end_point: Point::new(usize::MAX, usize::MAX),
    }
}

fn overlaps(a: &[Range], b: &[Range]) -> bool {
    let extent =
        |r: &[Range]| r.first().map_or(0, |r| r.start_byte)..r.last().map_or(0, |r| r.end_byte);
    let (a, b) = (extent(a), extent(b));
    a.start <= b.end && b.start <= a.end
}

/// `HighlightIterLayer::intersect_ranges`: the parts of `nodes` (without
/// their children unless `includes_children`) inside `parent_ranges`.
fn intersect_ranges(
    parent_ranges: &[Range],
    nodes: &[Node],
    includes_children: bool,
) -> Vec<Range> {
    let mut result = Vec::new();
    let Some(first) = nodes.first() else {
        return result;
    };
    let mut cursor = first.walk();
    let mut parent_range_iter = parent_ranges.iter();
    let Some(mut parent_range) = parent_range_iter.next() else {
        return result;
    };
    for node in nodes {
        let mut preceding_range = Range {
            start_byte: 0,
            start_point: Point::new(0, 0),
            end_byte: node.start_byte(),
            end_point: node.start_position(),
        };
        let following_range = Range {
            start_byte: node.end_byte(),
            start_point: node.end_position(),
            end_byte: usize::MAX,
            end_point: Point::new(usize::MAX, usize::MAX),
        };
        let children: Vec<Range> = if includes_children {
            Vec::new()
        } else {
            node.children(&mut cursor)
                .map(|child| child.range())
                .collect()
        };
        for excluded_range in children.into_iter().chain(std::iter::once(following_range)) {
            let mut range = Range {
                start_byte: preceding_range.end_byte,
                start_point: preceding_range.end_point,
                end_byte: excluded_range.start_byte,
                end_point: excluded_range.start_point,
            };
            preceding_range = excluded_range;

            if range.end_byte < parent_range.start_byte {
                continue;
            }
            while parent_range.start_byte <= range.end_byte {
                if parent_range.end_byte > range.start_byte {
                    if range.start_byte < parent_range.start_byte {
                        range.start_byte = parent_range.start_byte;
                        range.start_point = parent_range.start_point;
                    }
                    if parent_range.end_byte < range.end_byte {
                        if range.start_byte < parent_range.end_byte {
                            result.push(Range {
                                start_byte: range.start_byte,
                                start_point: range.start_point,
                                end_byte: parent_range.end_byte,
                                end_point: parent_range.end_point,
                            });
                        }
                        range.start_byte = parent_range.end_byte;
                        range.start_point = parent_range.end_point;
                    } else {
                        if range.start_byte < range.end_byte {
                            result.push(range);
                        }
                        break;
                    }
                }
                match parent_range_iter.next() {
                    Some(next) => parent_range = next,
                    None => return result,
                }
            }
        }
    }
    result
}

/// Move a byte range along an edit as [`edit_range`] does.
pub(crate) fn edit_bytes(bytes: &mut Bytes<usize>, edit: &InputEdit) {
    let map = |byte: usize| {
        if byte >= edit.old_end_byte {
            edit.new_end_byte + (byte - edit.old_end_byte)
        } else {
            byte.min(edit.start_byte)
        }
    };
    *bytes = map(bytes.start)..map(bytes.end);
}

/// Move `range` along an edit the way `Tree::edit` moves a tree's included
/// ranges (`ts_range_edit`).
fn edit_range(range: &mut Range, edit: &InputEdit) {
    if range.end_byte >= edit.old_end_byte {
        if range.end_byte != usize::MAX {
            range.end_byte = edit.new_end_byte + (range.end_byte - edit.old_end_byte);
            range.end_point = point_add(
                edit.new_end_position,
                point_sub(range.end_point, edit.old_end_position),
            );
        }
    } else if range.end_byte > edit.start_byte {
        range.end_byte = edit.start_byte;
        range.end_point = edit.start_position;
    }
    if range.start_byte >= edit.old_end_byte {
        range.start_byte = edit.new_end_byte + (range.start_byte - edit.old_end_byte);
        range.start_point = point_add(
            edit.new_end_position,
            point_sub(range.start_point, edit.old_end_position),
        );
    } else if range.start_byte > edit.start_byte {
        range.start_byte = edit.start_byte;
        range.start_point = edit.start_position;
    }
}

fn point_add(a: Point, b: Point) -> Point {
    if b.row > 0 {
        Point::new(a.row + b.row, b.column)
    } else {
        Point::new(a.row, a.column + b.column)
    }
}

fn point_sub(a: Point, b: Point) -> Point {
    if a.row > b.row {
        Point::new(a.row - b.row, a.column)
    } else {
        Point::new(0, a.column.saturating_sub(b.column))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LanguageQueries, HIGHLIGHT_NAMES};
    use ratatui::style::Color;
    use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

    /// A JavaScript-parsed language with queries of the test's choosing.
    struct Lang {
        name: &'static str,
        config: &'static HighlightConfiguration,
        queries: &'static LanguageQueries,
    }

    impl Lang {
        fn new(name: &'static str, highlights: &str, injections: &str) -> Self {
            let names: Vec<String> = HIGHLIGHT_NAMES.iter().map(|s| s.to_string()).collect();
            let language = tree_sitter_javascript::LANGUAGE.into();
            let mut config =
                HighlightConfiguration::new(language, name, highlights, injections, "").unwrap();
            config.configure(&names);
            let config = Box::leak(Box::new(config));
            let queries = LanguageQueries::new(config, injections, &names).unwrap();
            Self {
                name,
                config,
                queries: Box::leak(Box::new(queries)),
            }
        }

        fn highlights(&self) -> QueryHighlights<'static> {
            self.queries.highlights(self.name, self.config)
        }
    }

    /// The highlight of every byte as `tree_sitter_highlight` has it.
    fn reference(host: &Lang, inner: &Lang, text: &str) -> Vec<Option<usize>> {
        let mut highlighter = Highlighter::new();
        let events = highlighter
            .highlight(host.config, text.as_bytes(), None, None, |name| {
                (name == inner.name).then_some(inner.config)
            })
            .unwrap();
        let mut bytes = vec![None; text.len()];
        let mut stack = Vec::new();
        for event in events {
            match event.unwrap() {
                HighlightEvent::Source { start, end } => {
                    bytes[start..end].fill(stack.last().copied());
                }
                HighlightEvent::HighlightStart(h) => stack.push(h.0),
                HighlightEvent::HighlightEnd => {
                    stack.pop();
                }
            }
        }
        bytes
    }

    /// The highlight of every byte in `window` as the replay has it.
    fn replayed(host: &Lang, inner: &Lang, text: &str, window: Bytes<usize>) -> Vec<Option<usize>> {
        let rope = Rope::from_str(text);
        let highlights = host.highlights();
        let tree = parse(&mut Parser::new(), highlights.language, &rope, None).unwrap();
        let resolve = |name: &str| (name == inner.name).then(|| inner.highlights());
        let mut cache = LayerCache::new();
        let combined = cache.host_combined_injections(&tree, &highlights, &rope, &resolve);
        let host_layer = HostLayer {
            tree: &tree,
            highlights,
            combined: combined.injections(),
        };
        let styles = |h: usize| Style::default().fg(Color::Indexed(h as u8));
        let runs = cache.highlight(
            &host_layer,
            &rope,
            window.clone(),
            &resolve,
            &styles,
            Style::default(),
        );
        let mut bytes = vec![None; window.len()];
        for (range, style) in runs {
            let highlight = match style.fg {
                Some(Color::Indexed(h)) => Some(h as usize),
                _ => None,
            };
            for byte in range.start.max(window.start)..range.end.min(window.end) {
                bytes[byte - window.start] = highlight;
            }
        }
        bytes
    }

    /// Every window of whole lines highlights as the full pass does.
    fn assert_windows_agree(host: &Lang, inner: &Lang, text: &str) {
        let expected = reference(host, inner, text);
        assert!(
            expected.iter().any(|h| h.is_some()),
            "the sample should be highlighted"
        );
        let mut starts: Vec<usize> = text.match_indices('\n').map(|(i, _)| i + 1).collect();
        starts.insert(0, 0);
        starts.retain(|&s| s < text.len());
        starts.push(text.len());
        for (i, &from) in starts.iter().enumerate() {
            for &to in &starts[i + 1..] {
                assert_eq!(
                    replayed(host, inner, text, from..to),
                    expected[from..to],
                    "window {from}..{to} of {text:?}"
                );
            }
        }
    }

    const TEMPLATES: &str = "let a = `first\nline ${b}\nthird`;\nlet c = `x` + d;\n";
    const INJECT_TEMPLATES: &str = r#"((template_string) @injection.content
  (#set! injection.language "inner")
  (#set! injection.include-children))"#;

    /// The host and the injected layer highlight the same node: the deeper
    /// layer's highlight stands, the host's is dropped.
    #[test]
    fn a_range_highlighted_by_two_layers_keeps_the_deeper_one() {
        let host = Lang::new(
            "host",
            "(template_string) @keyword\n(identifier) @variable",
            INJECT_TEMPLATES,
        );
        let inner = Lang::new(
            "inner",
            "(template_string) @string\n(identifier) @function",
            "",
        );
        assert_windows_agree(&host, &inner, TEMPLATES);
    }

    /// An injected highlight that starts with a host one but ends first
    /// closes the host's on the shared stack, so its own style runs on to
    /// the host's end. A window that starts in there needs the replay to
    /// begin before both.
    #[test]
    fn overlapping_highlights_of_two_layers_close_as_the_full_pass_does() {
        let host = Lang::new(
            "host",
            "(template_string) @keyword\n(identifier) @variable",
            INJECT_TEMPLATES,
        );
        let inner = Lang::new(
            "inner",
            "\"`\" @punctuation.bracket\n(template_string) @string",
            "",
        );
        assert_windows_agree(&host, &inner, TEMPLATES);
    }

    /// Combined injections parse the content of every match as one tree.
    #[test]
    fn combined_injections_highlight_as_one_tree() {
        let host = Lang::new(
            "host",
            "(string) @string\n(identifier) @variable",
            r#"((template_string (string_fragment) @injection.content)
  (#set! injection.language "inner")
  (#set! injection.combined))"#,
        );
        let inner = Lang::new("inner", "(identifier) @function\n(number) @number", "");
        let text = "f(`one 1`);\nlet s = \"q\";\ng(`two\n2 three`);\n";
        assert_windows_agree(&host, &inner, text);
    }
}
