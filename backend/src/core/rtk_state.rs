//! RTK's own state, made visible in Kronn — KT-197.
//!
//! RTK compresses very well where it is used; the residual is ADOPTION, and
//! adoption is invisible unless someone looks. `rtk gain`, `session`, `discover`,
//! `hook-audit` and `cc-economics` each know a piece of it, and each is a command
//! nobody runs unprompted.
//!
//! This turns their five outputs into one compact state. Two rules shape it:
//!
//! A SOURCE THAT CANNOT ANSWER SAYS WHY, AND WHAT TO DO. `hook-audit` with no log
//! and `cc-economics` against a drifted ccusage schema both produce nothing
//! useful; reported as empty they would read as "nothing to report", which is the
//! opposite of the truth. Each carries a diagnosis and a remedy.
//!
//! AND IT IS BOUNDED. A panel that pastes five command outputs into a context has
//! spent more than the adoption it was measuring.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Cap on the whole rendered state.
pub const RTK_STATE_MAX_BYTES: usize = 4_096;

const _: () = assert!(
    RTK_STATE_MAX_BYTES <= 8_192,
    "an RTK adoption panel above 8 KiB costs more than the adoption it reports"
);

/// One source's contribution, or a stated reason it has none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum SourceState {
    Ready {
        /// One line, already compact.
        summary: String,
        /// The figures worth charting, when the source gives numbers.
        metrics: Vec<Metric>,
    },
    /// Cannot answer. Both fields are required: a diagnosis with no remedy leaves
    /// a reader informed and stuck.
    Unavailable { diagnosis: String, remedy: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Metric {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RtkState {
    pub gain: SourceState,
    pub session: SourceState,
    pub discover: SourceState,
    pub hook_audit: SourceState,
    pub cc_economics: SourceState,
}

/// Which RTK command a piece of output came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RtkSource {
    Gain,
    Session,
    Discover,
    HookAudit,
    CcEconomics,
}

impl RtkSource {
    /// The Quick Exec template that produces this source.
    pub fn template_id(self) -> &'static str {
        match self {
            Self::Gain => "rtk-gain",
            Self::Session => "rtk-session",
            Self::Discover => "rtk-discover",
            Self::HookAudit => "rtk-hook-audit",
            Self::CcEconomics => "rtk-cc-economics",
        }
    }
}

/// Turn one command's output into a state.
///
/// `ran` is false when the command never produced a usable result — it timed out,
/// was cancelled, or `rtk` is not installed. That case is `Unavailable` rather
/// than an empty `Ready`: an unmeasured adoption rate is not a good one.
pub fn classify(source: RtkSource, stdout: &str, stderr: &str, ran: bool) -> SourceState {
    if !ran {
        return SourceState::Unavailable {
            diagnosis: format!(
                "`rtk {}` produced no usable result",
                source.template_id().trim_start_matches("rtk-")
            ),
            remedy: "check that rtk is installed and on PATH, then re-run the collection"
                .to_string(),
        };
    }

    // Checked before the parsers: these tools exit 0 while telling you they have
    // nothing, so a parser reading a zero out of them would report a real figure.
    if let Some(known) = known_blocker(source, stdout, stderr) {
        return known;
    }

    match source {
        RtkSource::Gain => parse_gain(stdout),
        RtkSource::Session => parse_session(stdout),
        RtkSource::Discover => parse_discover(stdout),
        RtkSource::HookAudit => parse_hook_audit(stdout),
        RtkSource::CcEconomics => parse_cc_economics(stdout),
    }
}

/// The failures observed in practice, each with the action that clears it.
fn known_blocker(source: RtkSource, stdout: &str, stderr: &str) -> Option<SourceState> {
    let both = format!("{stdout}\n{stderr}");

    if both.contains("No audit log found") {
        return Some(SourceState::Unavailable {
            diagnosis: "hook rewrite auditing is off, so there is no record of what \
                        RTK rewrote"
                .to_string(),
            remedy: "export RTK_HOOK_AUDIT=1 in the shell that launches the CLI, then \
                     use it once before reading this again"
                .to_string(),
        });
    }

    // ccusage's monthly payload changed shape; rtk 0.42.4 still expects the old
    // one. Named precisely, because "cc-economics failed" sends someone reading
    // rtk's source instead of ccusage's release notes.
    if both.contains("Failed to parse ccusage JSON") || both.contains("Invalid JSON structure") {
        let field = missing_field(&both);
        let detail = match &field {
            Some(name) => format!(
                "ccusage's monthly payload no longer carries `{name}`, which this rtk \
                 build requires"
            ),
            None => {
                "ccusage's payload no longer matches the shape this rtk build expects".to_string()
            }
        };
        return Some(SourceState::Unavailable {
            diagnosis: format!("{detail} — spend cannot be paired with savings"),
            remedy: "pin ccusage to the version rtk was built against, or update rtk; \
                     `rtk gain` alone still reports savings, without the spend side"
                .to_string(),
        });
    }

    if both.contains("ccusage not installed") && both.contains("npx") {
        return Some(SourceState::Unavailable {
            diagnosis: "ccusage is not installed, so rtk reached for it through a \
                        package runner"
                .to_string(),
            remedy: "install ccusage globally: fetching it per call is slow and, in a \
                     worktree, a package runner can rewrite the main checkout's \
                     node_modules"
                .to_string(),
        });
    }

    if source == RtkSource::CcEconomics && both.trim().is_empty() {
        return Some(SourceState::Unavailable {
            diagnosis: "cc-economics returned nothing at all".to_string(),
            remedy: "run `rtk cc-economics` by hand to see the error it swallowed".to_string(),
        });
    }

    None
}

/// `missing field `month`` → `month`.
fn missing_field(text: &str) -> Option<String> {
    let after = text.split("missing field").nth(1)?;
    let start = after.find('`')? + 1;
    let rest = &after[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

fn parse_gain(stdout: &str) -> SourceState {
    let commands = labelled(stdout, "Total commands:");
    let saved = labelled(stdout, "Tokens saved:");
    match (commands, saved) {
        (Some(commands), Some(saved)) => SourceState::Ready {
            summary: format!("{saved} saved over {commands} commands"),
            metrics: vec![
                Metric {
                    label: "commands".into(),
                    value: commands,
                },
                Metric {
                    label: "saved".into(),
                    value: saved,
                },
            ],
        },
        // Parsed nothing, so say that rather than reporting zero savings.
        _ => SourceState::Unavailable {
            diagnosis: "`rtk gain` output did not carry the totals this build reads".to_string(),
            remedy: "check the rtk version; the summary format changed".to_string(),
        },
    }
}

fn parse_session(stdout: &str) -> SourceState {
    let average = labelled(stdout, "Average adoption:");
    // Sessions are the lines with a percentage in the adoption column.
    let sessions = stdout
        .lines()
        .filter(|line| line.contains('%') && !line.contains("Average"))
        .count();
    match average {
        Some(average) => SourceState::Ready {
            summary: format!("{average} adoption across {sessions} session(s)"),
            metrics: vec![
                Metric {
                    label: "adoption".into(),
                    value: average,
                },
                Metric {
                    label: "sessions".into(),
                    value: sessions.to_string(),
                },
            ],
        },
        None => SourceState::Unavailable {
            diagnosis: "`rtk session` reported no adoption figure".to_string(),
            remedy: "run a CLI session first; adoption needs recorded commands to \
                     measure"
                .to_string(),
        },
    }
}

fn parse_discover(stdout: &str) -> SourceState {
    // "No missed opportunities" is a real, good answer — distinct from a source
    // that could not look.
    if stdout.contains("No missed") || stdout.contains("no missed") {
        return SourceState::Ready {
            summary: "no missed RTK opportunity found".to_string(),
            metrics: vec![Metric {
                label: "misses".into(),
                value: "0".into(),
            }],
        };
    }
    let misses = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with(|c: char| c.is_ascii_digit()))
        .count();
    SourceState::Ready {
        summary: format!("{misses} line(s) of missed RTK opportunities"),
        metrics: vec![Metric {
            label: "misses".into(),
            value: misses.to_string(),
        }],
    }
}

fn parse_hook_audit(stdout: &str) -> SourceState {
    let rewrites = labelled(stdout, "Rewrites:").or_else(|| labelled(stdout, "rewrites:"));
    match rewrites {
        Some(rewrites) => SourceState::Ready {
            summary: format!("{rewrites} hook rewrite(s) recorded"),
            metrics: vec![Metric {
                label: "rewrites".into(),
                value: rewrites,
            }],
        },
        None => SourceState::Unavailable {
            diagnosis: "the hook audit log exists but carries no rewrite count".to_string(),
            remedy: "use the CLI once with RTK_HOOK_AUDIT=1 set, then read again".to_string(),
        },
    }
}

fn parse_cc_economics(stdout: &str) -> SourceState {
    let spend = labelled(stdout, "Spending:").or_else(|| labelled(stdout, "Total spend:"));
    match spend {
        Some(spend) => SourceState::Ready {
            summary: format!("spend {spend}, paired with rtk savings"),
            metrics: vec![Metric {
                label: "spend".into(),
                value: spend,
            }],
        },
        None => SourceState::Unavailable {
            diagnosis: "cc-economics produced output without a spend figure".to_string(),
            remedy: "compare it against `rtk gain`, which reports savings without \
                     needing ccusage"
                .to_string(),
        },
    }
}

/// The text after a `Label:` on its line, trimmed.
fn labelled(text: &str, label: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.split_once(label))
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Render the state as the text a reader gets.
///
/// Unavailable sources come FIRST. They are the actionable part — each one names
/// something that is not being measured — and a truncation must not be able to
/// drop them in favour of figures that are already fine.
pub fn render(state: &RtkState) -> String {
    let sources = [
        ("gain", &state.gain),
        ("session", &state.session),
        ("discover", &state.discover),
        ("hook-audit", &state.hook_audit),
        ("cc-economics", &state.cc_economics),
    ];

    let mut out = String::from("RTK ADOPTION\n");

    let blocked: Vec<_> = sources
        .iter()
        .filter(|(_, state)| matches!(state, SourceState::Unavailable { .. }))
        .collect();
    if !blocked.is_empty() {
        out.push_str(&format!("\nNOT MEASURED ({}):\n", blocked.len()));
        for (name, state) in &blocked {
            if let SourceState::Unavailable { diagnosis, remedy } = state {
                out.push_str(&format!("- {name}: {diagnosis}\n  → {remedy}\n"));
            }
        }
    }

    let ready: Vec<_> = sources
        .iter()
        .filter(|(_, state)| matches!(state, SourceState::Ready { .. }))
        .collect();
    if ready.is_empty() {
        // Said explicitly: an empty panel would read as "adoption is fine".
        out.push_str("\nNo source could be read, so adoption is UNKNOWN.\n");
    } else {
        out.push_str("\nMEASURED:\n");
        for (name, state) in &ready {
            if let SourceState::Ready { summary, .. } = state {
                out.push_str(&format!("- {name}: {summary}\n"));
            }
        }
    }

    if out.len() > RTK_STATE_MAX_BYTES {
        let mut cut = RTK_STATE_MAX_BYTES;
        while cut > 0 && !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n… truncated\n");
    }
    out
}

#[cfg(test)]
#[path = "rtk_state_test.rs"]
mod rtk_state_test;
