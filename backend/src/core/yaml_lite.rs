//! A small reader for the YAML a CI file is written in (KT-1404).
//!
//! The strict parser (`serde_yaml`) is a dev-dependency only; a read of a
//! workflow does not justify a runtime one. This covers what workflows and
//! pipelines use: block mappings and sequences, `- key: value` items, single
//! quoted / double quoted / plain scalars, flow sequences and maps, comments,
//! and `|` / `>` block scalars (their text is dropped). Anchors, tags and
//! multi-document streams are not interpreted. Anchors (`&name`), aliases
//! (`*name`) and merge keys (`<<: *name`) are resolved when the anchor is
//! defined earlier in the file; tags are dropped. What it cannot read becomes a
//! missing node, never a guess, and the lines it had to skip are returned by
//! [`parse_checked`] so a caller never presents a partial read as an absence.
//! A file it cannot read at all is `None`.
//!
//! Every key and scalar is kept as written (`8.10` stays `8.10`) with the line
//! of each mapping key, so a fact can cite `file:line`.

/// A YAML value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Scalar(String),
    Seq(Vec<Node>),
    Map(Vec<Entry>),
}

/// One `key: value` of a mapping, with the 1-based line of the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub line: usize,
    pub value: Node,
}

impl Node {
    pub fn entry(&self, key: &str) -> Option<&Entry> {
        match self {
            Node::Map(entries) => entries.iter().find(|entry| entry.key == key),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entry(key).map(|entry| &entry.value)
    }

    pub fn scalar(&self) -> Option<&str> {
        match self {
            Node::Scalar(text) => Some(text),
            _ => None,
        }
    }

    pub fn items(&self) -> &[Node] {
        match self {
            Node::Seq(items) => items,
            _ => &[],
        }
    }

    pub fn entries(&self) -> &[Entry] {
        match self {
            Node::Map(entries) => entries,
            _ => &[],
        }
    }

    /// The node flattened into flow style: `[a, b]`, `{k: v}`, `text`.
    pub fn flow(&self) -> String {
        match self {
            Node::Scalar(text) => text.clone(),
            Node::Seq(items) => format!(
                "[{}]",
                items.iter().map(Node::flow).collect::<Vec<_>>().join(", ")
            ),
            Node::Map(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|e| format!("{}: {}", e.key, e.value.flow()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct Line {
    indent: usize,
    text: String,
    number: usize,
}

/// A line without its comment: a `#` opens one at the start or after a space,
/// outside quotes.
fn strip_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let (mut single, mut double) = (false, false);
    let mut previous = ' ';
    for c in line.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single && !double && (previous == ' ' || previous == '\t') => break,
            _ => {}
        }
        out.push(c);
        previous = c;
    }
    out.trim_end().to_string()
}

fn lines_of(text: &str) -> Vec<Line> {
    text.lines()
        .enumerate()
        .filter_map(|(n, raw)| {
            let stripped = strip_comment(raw);
            let trimmed = stripped.trim_start();
            if trimmed.is_empty() || trimmed == "---" || trimmed == "..." {
                return None;
            }
            Some(Line {
                indent: stripped.len() - trimmed.len(),
                text: trimmed.to_string(),
                number: n + 1,
            })
        })
        .collect()
}

/// The position of the `:` that ends a mapping key: followed by a space or the
/// end of the line, outside quotes and flow brackets.
fn key_colon(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    if text.starts_with(['[', '{']) {
        return None;
    }
    let (mut single, mut double) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'\'' if !double => single = !single,
            b'"' if !single => double = !double,
            b':' if !single && !double && (i + 1 == bytes.len() || bytes[i + 1] == b' ') => {
                return Some(i);
            }
            _ => {}
        }
    }
    None
}

fn unquote(text: &str) -> String {
    let t = text.trim();
    for quote in ['"', '\''] {
        if t.len() >= 2 && t.starts_with(quote) && t.ends_with(quote) {
            return t[1..t.len() - 1].to_string();
        }
    }
    t.to_string()
}

/// Splits at the commas of depth 0.
fn split_flow(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let (mut depth, mut single, mut double) = (0i32, false, false);
    for c in inner.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '[' | '{' if !single && !double => depth += 1,
            ']' | '}' if !single && !double => depth -= 1,
            ',' if depth == 0 && !single && !double => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

fn flow_node(text: &str, line: usize) -> Node {
    let t = text.trim();
    if let Some(inner) = t.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        return Node::Seq(
            split_flow(inner)
                .iter()
                .map(|part| flow_node(part, line))
                .collect(),
        );
    }
    if let Some(inner) = t.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        let entries = split_flow(inner)
            .iter()
            .filter_map(|part| {
                let part = part.trim();
                let at = key_colon(part).or_else(|| part.find(':'))?;
                Some(Entry {
                    key: unquote(&part[..at]),
                    line,
                    value: flow_node(&part[at + 1..], line),
                })
            })
            .collect();
        return Node::Map(entries);
    }
    Node::Scalar(unquote(t))
}

fn brackets_balanced(text: &str) -> bool {
    let (mut depth, mut single, mut double) = (0i32, false, false);
    for c in text.chars() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '[' | '{' if !single && !double => depth += 1,
            ']' | '}' if !single && !double => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

struct Reader {
    lines: Vec<Line>,
    at: usize,
    anchors: std::collections::HashMap<String, Node>,
    /// Lines the reader could not place: the read around them is partial.
    unread: Vec<usize>,
}

/// Anchor and tag properties at the start of a value: `&name`, `!tag`.
/// Returns the anchor (if any) and the text that follows.
fn take_props(text: &str) -> (Option<String>, &str) {
    let mut rest = text.trim_start();
    let mut anchor = None;
    loop {
        let token_end = rest.find([' ', '\t']).unwrap_or(rest.len());
        let token = &rest[..token_end];
        if let Some(name) = token.strip_prefix('&').filter(|n| !n.is_empty()) {
            anchor = Some(name.to_string());
        } else if !(token.starts_with('!') && token.len() > 1) {
            break;
        }
        rest = rest[token_end..].trim_start();
    }
    (anchor, rest)
}

impl Reader {
    fn peek(&self) -> Option<&Line> {
        self.lines.get(self.at)
    }

    /// Skips the lines of a `|` / `>` block scalar: those indented deeper than `parent`.
    fn skip_block_scalar(&mut self, parent: usize) {
        while self.peek().is_some_and(|line| line.indent > parent) {
            self.at += 1;
        }
    }

    /// The value after `key:` (or after `- `): `rest` is the text on the same
    /// line, `indent` the indentation of the key or dash.
    fn value(&mut self, rest: &str, indent: usize, line: usize, dash: bool) -> Node {
        let (anchor, rest) = take_props(rest);
        let node = self.value_inner(rest.trim(), indent, line, dash);
        if let Some(name) = anchor {
            self.anchors.insert(name, node.clone());
        }
        node
    }

    /// `*name` as the node of its anchor; an alias defined nowhere is unread.
    fn alias(&mut self, text: &str, line: usize) -> Option<Node> {
        let name = text.strip_prefix('*')?;
        if name.is_empty() || name.contains(char::is_whitespace) {
            return None;
        }
        let found = self.anchors.get(name).cloned();
        if found.is_none() {
            self.unread.push(line);
        }
        Some(found.unwrap_or_else(|| Node::Scalar(String::new())))
    }

    fn value_inner(&mut self, rest: &str, indent: usize, line: usize, dash: bool) -> Node {
        if let Some(node) = self.alias(rest, line) {
            return node;
        }
        if rest.is_empty() {
            return match self.peek() {
                Some(next) if next.indent > indent => self.block(next.indent),
                // `key:` followed by a sequence at the key's own indentation.
                Some(next) if !dash && next.indent == indent && is_dash(&next.text) => {
                    self.block(indent)
                }
                _ => Node::Scalar(String::new()),
            };
        }
        if rest.starts_with(['|', '>']) {
            self.skip_block_scalar(indent);
            return Node::Scalar("…".to_string());
        }
        if rest.starts_with(['[', '{']) {
            let mut joined = rest.to_string();
            while !brackets_balanced(&joined) {
                let Some(next) = self.peek() else { break };
                joined.push(' ');
                joined.push_str(&next.text);
                self.at += 1;
            }
            return flow_node(&joined, line);
        }
        // A plain scalar may continue on the deeper lines that follow.
        let mut text = rest.to_string();
        while self
            .peek()
            .is_some_and(|next| next.indent > indent && !text.starts_with(['"', '\'']))
        {
            let next = &self.lines[self.at];
            text.push(' ');
            text.push_str(&next.text);
            self.at += 1;
        }
        Node::Scalar(unquote(&text))
    }

    fn block(&mut self, indent: usize) -> Node {
        match self.peek() {
            Some(line) if line.indent == indent && is_dash(&line.text) => self.sequence(indent),
            Some(_) => self.mapping(indent),
            None => Node::Scalar(String::new()),
        }
    }

    fn sequence(&mut self, indent: usize) -> Node {
        let mut items = Vec::new();
        while let Some(line) = self.peek().cloned() {
            if line.indent != indent || !is_dash(&line.text) {
                break;
            }
            let (anchor, rest) = take_props(line.text[1..].trim_start());
            let offset = line.text.len() - rest.len();
            let before = items.len();
            if rest.is_empty() {
                self.at += 1;
                items.push(match self.peek() {
                    Some(next) if next.indent > indent => self.block(next.indent),
                    _ => Node::Scalar(String::new()),
                });
            } else if key_colon(rest).is_some() && !rest.starts_with(['"', '\'']) {
                // `- key: value`: the entries of an inline mapping sit at the
                // column of `key`.
                self.lines[self.at] = Line {
                    indent: indent + offset,
                    text: rest.to_string(),
                    number: line.number,
                };
                items.push(self.mapping(indent + offset));
            } else if is_dash(rest) {
                // `- - nested`: a sequence in a sequence.
                self.lines[self.at] = Line {
                    indent: indent + offset,
                    text: rest.to_string(),
                    number: line.number,
                };
                items.push(self.sequence(indent + offset));
            } else {
                self.at += 1;
                items.push(self.value(rest, indent, line.number, true));
            }
            if let (Some(name), Some(item)) = (anchor, items.get(before)) {
                self.anchors.insert(name, item.clone());
            }
        }
        Node::Seq(items)
    }

    fn mapping(&mut self, indent: usize) -> Node {
        let mut entries: Vec<Entry> = Vec::new();
        let mut merged: Vec<Entry> = Vec::new();
        while let Some(line) = self.peek().cloned() {
            if line.indent < indent || (line.indent == indent && is_dash(&line.text)) {
                break;
            }
            if line.indent > indent {
                // Deeper than the key above it owns: not placed, so the read is partial.
                self.unread.push(line.number);
                self.at += 1;
                continue;
            }
            let Some(colon) = key_colon(&line.text) else {
                // Not a `key:` line (a stray scalar): skipped, and said so.
                self.unread.push(line.number);
                self.at += 1;
                continue;
            };
            let key = unquote(&line.text[..colon]);
            let rest = line.text[colon + 1..].to_string();
            self.at += 1;
            let value = self.value(&rest, indent, line.number, false);
            if key == "<<" {
                // Merge key: the entries of each source, the first source winning;
                // a key written in the mapping itself wins over all of them.
                let sources = match value {
                    Node::Seq(items) => items,
                    other => vec![other],
                };
                for source in sources {
                    let source = match source {
                        Node::Scalar(text) if text.starts_with('*') => self
                            .alias(&text, line.number)
                            .unwrap_or(Node::Scalar(String::new())),
                        other => other,
                    };
                    for entry in source.entries() {
                        if !merged.iter().any(|e| e.key == entry.key) {
                            merged.push(entry.clone());
                        }
                    }
                }
                continue;
            }
            entries.push(Entry {
                key,
                line: line.number,
                value,
            });
        }
        for entry in merged {
            if !entries.iter().any(|e| e.key == entry.key) {
                entries.push(entry);
            }
        }
        Node::Map(entries)
    }
}

fn is_dash(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

/// Parses `text`; `None` when it holds nothing readable.
pub fn parse(text: &str) -> Option<Node> {
    parse_checked(text).map(|(node, _)| node)
}

/// Like [`parse`], with the sorted 1-based numbers of the lines the reader had
/// to skip. Empty when the whole file was read.
pub fn parse_checked(text: &str) -> Option<(Node, Vec<usize>)> {
    let lines = lines_of(text);
    let first = lines.first()?.indent;
    let mut reader = Reader {
        lines,
        at: 0,
        anchors: std::collections::HashMap::new(),
        unread: Vec::new(),
    };
    let node = reader.block(first);
    // Lines left after the root block ended are unplaced too.
    while let Some(line) = reader.peek() {
        reader.unread.push(line.number);
        reader.at += 1;
    }
    let mut unread = reader.unread;
    unread.sort_unstable();
    unread.dedup();
    match &node {
        Node::Map(entries) if entries.is_empty() => None,
        Node::Seq(items) if items.is_empty() => None,
        Node::Scalar(_) => None,
        _ => Some((node, unread)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workflow_is_read_with_the_lines_of_its_keys() {
        let text = "name: CI # the name\non:\n  pull_request:\n    types: [labeled, synchronize]\n  push:\n    branches:\n      - main\n      - 'release/*'\njobs:\n  test:\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n      - run: |\n          echo one: two\n          echo three\n      - name: last\n        run: npm test\n";
        let root = parse(text).unwrap();
        assert_eq!(root.get("name").and_then(Node::scalar), Some("CI"));
        let on = root.entry("on").unwrap();
        assert_eq!(on.line, 2);
        let types = on.value.get("pull_request").unwrap().get("types").unwrap();
        assert_eq!(types.flow(), "[labeled, synchronize]");
        assert_eq!(
            on.value
                .get("push")
                .unwrap()
                .get("branches")
                .unwrap()
                .flow(),
            "[main, release/*]"
        );
        let job = root.get("jobs").unwrap().entry("test").unwrap();
        assert_eq!(job.line, 10);
        assert_eq!(
            job.value.get("if").and_then(Node::scalar),
            Some("contains(github.event.pull_request.labels.*.name, 'ci-test')")
        );
        let steps = job.value.get("steps").unwrap().items();
        assert_eq!(steps.len(), 3, "{steps:?}");
        let with = steps[0].get("with").unwrap().entry("node-version").unwrap();
        assert_eq!((with.value.scalar(), with.line), (Some("20"), 15));
        // A block scalar's lines are not entries of the step.
        assert_eq!(steps[1].entries().len(), 1);
        assert_eq!(steps[2].get("run").and_then(Node::scalar), Some("npm test"));
    }

    #[test]
    fn sequences_at_the_key_indentation_and_flow_maps_are_read() {
        let text = "schedule:\n- cron: '0 3 * * *'\n- cron: \"0 4 * * 1\"\nmatrix:\n  include: [{node: 18, os: linux}, {node: 20}]\n  php: ['8.1', 8.10]\n";
        let root = parse(text).unwrap();
        let crons: Vec<&str> = root
            .get("schedule")
            .unwrap()
            .items()
            .iter()
            .filter_map(|item| item.get("cron").and_then(Node::scalar))
            .collect();
        assert_eq!(crons, vec!["0 3 * * *", "0 4 * * 1"]);
        let matrix = root.get("matrix").unwrap();
        assert_eq!(matrix.get("include").unwrap().items().len(), 2);
        assert_eq!(matrix.get("php").unwrap().flow(), "[8.1, 8.10]");
    }

    #[test]
    fn urls_quoted_colons_and_hashes_are_not_mistaken_for_syntax() {
        let text = "image: 'node:20-alpine'\nurl: https://example.org/a#b\nrun: \"echo a: b # not a comment\"\nexpr: ${{ matrix.node }}\n";
        let root = parse(text).unwrap();
        assert_eq!(
            root.get("image").and_then(Node::scalar),
            Some("node:20-alpine")
        );
        assert_eq!(
            root.get("url").and_then(Node::scalar),
            Some("https://example.org/a#b")
        );
        assert_eq!(
            root.get("run").and_then(Node::scalar),
            Some("echo a: b # not a comment")
        );
        assert_eq!(
            root.get("expr").and_then(Node::scalar),
            Some("${{ matrix.node }}")
        );
    }

    #[test]
    fn an_anchor_with_a_nested_mapping_does_not_cut_the_job_short() {
        let text = "jobs:\n  build:\n    env: &defaults\n      NODE_ENV: test\n      CI: true\n    if: contains(github.event.pull_request.labels.*.name, 'ci-test')\n    steps:\n      - uses: actions/setup-node@v4\n        with:\n          node-version: 20\n  lint:\n    env: *defaults\n    steps: []\n";
        let (root, unread) = parse_checked(text).unwrap();
        assert!(unread.is_empty(), "{unread:?}");
        let build = root.get("jobs").unwrap().get("build").unwrap();
        assert_eq!(
            build.get("env").unwrap().get("CI").and_then(Node::scalar),
            Some("true")
        );
        assert!(build.get("if").is_some(), "{build:?}");
        assert!(build.get("steps").is_some(), "{build:?}");
        let lint = root.get("jobs").unwrap().get("lint").unwrap();
        assert_eq!(
            lint.get("env")
                .unwrap()
                .get("NODE_ENV")
                .and_then(Node::scalar),
            Some("test")
        );
    }

    #[test]
    fn merge_keys_and_tags_are_resolved_and_an_unknown_alias_is_reported() {
        let text = "base: &base\n  runs-on: x\n  timeout: 5\njob:\n  <<: *base\n  timeout: 9\n  tag: !!str 20\n  other: *nowhere\n";
        let (root, unread) = parse_checked(text).unwrap();
        let job = root.get("job").unwrap();
        assert_eq!(job.get("runs-on").and_then(Node::scalar), Some("x"));
        assert_eq!(job.get("timeout").and_then(Node::scalar), Some("9"));
        assert_eq!(job.get("tag").and_then(Node::scalar), Some("20"));
        assert_eq!(unread, vec![8], "an alias to nothing is a partial read");
    }

    #[test]
    fn a_line_the_reader_cannot_place_is_reported_not_swallowed() {
        let (_, unread) = parse_checked("a:\n  b: 1\n    c: 2\n  d: 3\n").unwrap();
        assert!(unread.is_empty(), "a plain scalar continues: {unread:?}");
        let (root, unread) = parse_checked("a:\n  - x\n  stray text\nb: 1\n").unwrap();
        assert_eq!(unread, vec![3]);
        assert_eq!(root.get("b").and_then(Node::scalar), Some("1"));
    }

    #[test]
    fn what_cannot_be_read_is_none_not_a_guess() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("# only a comment\n"), None);
        assert_eq!(parse("just a scalar"), None);
    }
}
