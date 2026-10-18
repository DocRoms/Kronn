//! KT-1341 — a built-in skill read section by section. The workflow
//! architect is the one guide every agent (MCP or HTTP) loads before building
//! a workflow or a Page; at ~120 KB it is served as a table of contents, then
//! the sections asked for.
use serde_json::{json, Value};

/// The built-in skill every agent loads before composing a workflow or a Page.
pub const WORKFLOW_GUIDE: &str = "workflow-architect";

struct Heading<'a> {
    level: usize,
    title: &'a str,
    start: usize,
}

fn headings(body: &str) -> Vec<Heading<'_>> {
    let mut found = Vec::new();
    let mut offset = 0;
    let mut fenced = false;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed.starts_with("```") {
            fenced = !fenced;
        }
        if !fenced {
            for (prefix, level) in [("## ", 2), ("### ", 3)] {
                if let Some(title) = trimmed.strip_prefix(prefix) {
                    found.push(Heading {
                        level,
                        title: title.trim(),
                        start: offset,
                    });
                }
            }
        }
        offset += line.len();
    }
    found
}

fn normalized(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The guide of a built-in skill: without `section`, its role, the table of
/// contents and how to read on; with one, that section (a `##` section with its
/// `###` sub-sections). `None` when no built-in skill has this id.
pub fn guide(id: &str, section: Option<&str>) -> Option<Value> {
    let skill = crate::core::skills::get_skill(id).filter(|skill| skill.is_builtin)?;
    let body = skill.content.as_str();
    let found = headings(body);
    let end_of = |index: usize| {
        let level = found[index].level;
        found[index + 1..]
            .iter()
            .find(|next| next.level <= level)
            .map_or(body.len(), |next| next.start)
    };
    let contents: Vec<Value> = found
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            json!({
                "section": heading.title,
                "level": heading.level,
                "bytes": end_of(index) - heading.start,
            })
        })
        .collect();
    let Some(wanted) = section.map(normalized).filter(|wanted| !wanted.is_empty()) else {
        // The role is the part before the first sub-section: small, and what
        // every reader needs first.
        let role_end = found.get(1).map_or(body.len(), |next| next.start);
        return Some(json!({
            "skill": id,
            "introduction": body[..role_end].trim(),
            "contents": contents,
            "next": "Call again with `section` set to a title from `contents`; read only the sections the task needs.",
        }));
    };
    let exact: Vec<usize> = (0..found.len())
        .filter(|&index| normalized(found[index].title) == wanted)
        .collect();
    let matches = if exact.is_empty() {
        (0..found.len())
            .filter(|&index| normalized(found[index].title).contains(&wanted))
            .collect()
    } else {
        exact
    };
    match matches.as_slice() {
        [index] => Some(json!({
            "skill": id,
            "section": found[*index].title,
            "content": body[found[*index].start..end_of(*index)].trim(),
        })),
        [] => Some(json!({
            "skill": id,
            "error": format!("no section matches `{}`", section.unwrap_or_default()),
            "contents": contents,
        })),
        several => Some(json!({
            "skill": id,
            "error": format!("`{}` matches several sections: name one", section.unwrap_or_default()),
            "candidates": several.iter().map(|&index| found[index].title).collect::<Vec<_>>(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workflow_guide_opens_on_its_contents_and_serves_one_section() {
        let first = guide(WORKFLOW_GUIDE, None).expect("built-in");
        assert!(first["introduction"]
            .as_str()
            .unwrap()
            .contains("Workflow Architect"));
        let titles: Vec<&str> = first["contents"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| entry["section"].as_str())
            .collect();
        assert!(
            titles.iter().any(|title| title.starts_with("Reach")),
            "{titles:?}"
        );
        // The opening stays small: the guide is read in parts.
        assert!(
            first.to_string().len() < 16_000,
            "{}",
            first.to_string().len()
        );

        let reach = guide(WORKFLOW_GUIDE, Some("reach")).unwrap();
        let content = reach["content"].as_str().unwrap();
        assert!(content.starts_with("## Reach"), "{content}");
        assert!(content.contains("Projects accessible") && content.contains("room_scope"));
        assert!(
            !content.contains("## Gotchas"),
            "a section stops at the next one"
        );
    }

    #[test]
    fn an_unknown_or_ambiguous_section_lists_what_exists() {
        let unknown = guide(WORKFLOW_GUIDE, Some("no such thing")).unwrap();
        assert!(unknown["error"].is_string() && unknown["contents"].is_array());
        assert!(guide("no-such-skill", None).is_none());
    }
}
