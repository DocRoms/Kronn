# Orchestration catalogue selection

Task launch and execution reassignment use `AgentSwitchPicker` for an explicit
agent-target choice and `ModelCatalogPicker` for the saved model catalogue.
The model override is cleared only by a successful explicit target change;
opening, searching, or a failed catalogue read leaves the stored value alone.
An override absent from the saved catalogue remains visible as unavailable so it
can be preserved rather than silently replaced. [src: file: frontend/src/components/TaskLaunchDialog.tsx:138-143]
[src: file: frontend/src/components/DiscussionPlanPanel.tsx:387-392]

The launch dialog must let picker-owned Escape handling finish first: the model
picker prevents the event, while the agent picker’s portal is recognised before
the dialog’s window-level close handler runs. [src: file: frontend/src/components/TaskLaunchDialog.tsx:96-104]
