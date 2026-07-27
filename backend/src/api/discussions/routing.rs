use crate::models::{AgentType, MessageTarget, MessageTargetKind};

/// One deterministic answer to “who owns the next reply?”.
///
/// Presence discovery stays in the HTTP/MCP adapters, but every adapter feeds
/// the same pure policy below. This keeps the human-send and joined-peer paths
/// from drifting as new room modes are added.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DispatchRoute {
    /// The room explicitly has no native responder.
    NoNativeResponder,
    /// One or more joined peers already own the reply.
    JoinedPeers,
    /// Start the discussion's configured native principal.
    NativePrincipal,
    /// Start exactly the explicitly requested agent as a one-shot override.
    TargetedNative(AgentType),
}

/// Route a human-authored turn.
///
/// Target identity, rather than transient presence, decides ownership:
/// an ordinary turn always belongs to the configured discussion agent, a
/// punctual `Agent` target launches that native agent once, and only an exact
/// `Cli` target belongs to a joined peer.
pub(crate) fn route_human_turn(
    no_agent_room: bool,
    target: Option<&MessageTarget>,
) -> DispatchRoute {
    if no_agent_room {
        return DispatchRoute::NoNativeResponder;
    }
    match target.map(|target| &target.kind) {
        Some(MessageTargetKind::Cli) => DispatchRoute::JoinedPeers,
        Some(MessageTargetKind::Agent) => {
            DispatchRoute::TargetedNative(target.expect("target exists").agent_type.clone())
        }
        Some(MessageTargetKind::DiscussionAgent) | None => DispatchRoute::NativePrincipal,
    }
}

/// Route a live single-message append from a joined agent.
///
/// Historical/bulk appends are rejected before this helper via
/// `is_live_peer_turn`. A no-agent room never starts a native process, even
/// for a target that is not currently joined: its contract is “joined peers
/// only”.
pub(crate) fn route_joined_peer_turn(
    is_live_peer_turn: bool,
    no_agent_room: bool,
    requested_target: Option<&AgentType>,
    target_is_eligible: bool,
    native_principal_is_eligible: bool,
) -> DispatchRoute {
    if !is_live_peer_turn || no_agent_room {
        return DispatchRoute::NoNativeResponder;
    }
    match requested_target {
        Some(_) if target_is_eligible => DispatchRoute::JoinedPeers,
        Some(agent) => DispatchRoute::TargetedNative(agent.clone()),
        None if native_principal_is_eligible => DispatchRoute::JoinedPeers,
        None => DispatchRoute::NativePrincipal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_routing_matrix_has_one_owner() {
        use DispatchRoute::*;

        let codex_agent = MessageTarget::agent(AgentType::Codex);
        let codex_principal = MessageTarget::discussion_agent(AgentType::Codex);
        let codex_cli = MessageTarget::cli(AgentType::Codex, 42);
        let cases = [
            (true, None, NoNativeResponder),
            (true, Some(&codex_agent), NoNativeResponder),
            (false, None, NativePrincipal),
            (false, Some(&codex_agent), TargetedNative(AgentType::Codex)),
            (false, Some(&codex_principal), NativePrincipal),
            (false, Some(&codex_cli), JoinedPeers),
        ];
        for (no_agent, target, expected) in cases {
            assert_eq!(route_human_turn(no_agent, target), expected);
        }
    }

    #[test]
    fn joined_peer_routing_matrix_has_one_owner() {
        use DispatchRoute::*;

        let cases = [
            (false, false, None, false, false, NoNativeResponder),
            (true, true, None, false, false, NoNativeResponder),
            (
                true,
                true,
                Some(AgentType::Codex),
                false,
                false,
                NoNativeResponder,
            ),
            (true, false, None, false, false, NativePrincipal),
            (true, false, None, false, true, JoinedPeers),
            (
                true,
                false,
                Some(AgentType::Codex),
                true,
                false,
                JoinedPeers,
            ),
            (
                true,
                false,
                Some(AgentType::Codex),
                false,
                true,
                TargetedNative(AgentType::Codex),
            ),
        ];
        for (live_turn, no_agent, target, target_live, native_live, expected) in cases {
            assert_eq!(
                route_joined_peer_turn(
                    live_turn,
                    no_agent,
                    target.as_ref(),
                    target_live,
                    native_live,
                ),
                expected
            );
        }
    }
}
