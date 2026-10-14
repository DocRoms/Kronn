# Briefing portability

`briefing.md` in the project's selected AI docs folder is authoritative for
the audit, the briefing editor and the project card. An empty or unreadable
file does not revive older DB notes. DB notes remain the fallback before the
file exists, so answers saved before template installation are usable.
[src: file: backend/src/api/projects/mod.rs:188-201#resolve_briefing_notes]
[src: file: backend/src/api/projects/mod.rs:224]
[src: file: backend/src/api/audit/briefing.rs:21-39#get_briefing]

Removing a TD-to-tasks or cross-agent option from the file disables it even
when the database still contains an older enabled option. Full audit launch
reads the resolved briefing once for both options.
[src: file: backend/src/api/audit/td_tasks.rs:36-40#briefing_option_set]
[src: file: backend/src/api/audit/cross_audit.rs:22-29#briefing_cross_agents]
[src: file: backend/src/api/audit/full.rs:309-317]

UI, MCP and workflow launches require saved briefing notes. A human can
explicitly choose the form's default answers; that saves “Not provided”
answers through the normal form endpoint. A workflow launch cannot make that
choice automatically.
[src: file: backend/src/api/audit/briefing_cards.rs:497-526#briefing_gate]
[src: file: frontend/src/components/BriefingForm.tsx:269-291#saveDefaults]

Phase 1 copies the resolved briefing into the selected docs folder immediately
after installing the templates, before the first agent step. A fresh clone
therefore carries its briefing and options without the original database.
When closed project answers are absent or unreadable, the project-doc hook
keeps an existing generated `project.md`; a newly installed template still
gets its default fields filled. The report exposes `kept_without_answers`.
[src: file: backend/src/api/audit/full.rs:791-829]
[src: file: backend/src/api/audit/project_doc.rs:1040-1047#ProjectDocReport]
[src: file: backend/src/api/audit/project_doc.rs:1082-1106#fill_project_doc_for_audit]

The regression suite covers HTTP reads and cards, stale database options,
empty/unreadable files, explicit workflow defaults, clone preservation and a
scripted Full audit that reads the briefing before its first step in both
`docs/` and `.ai/`.
[src: file: backend/tests/api_tests.rs:7829-7880#briefing_file_drives_http_reads_and_audited_project_cards]
[src: file: backend/src/api/audit/briefing_residuals_tests.rs:1]
[src: file: backend/src/api/audit/briefing_cards.rs:1049-1089#briefing_residuals_unattended_launch_requires_explicit_saved_defaults]
[src: file: backend/src/api/audit/relocation_e2e_tests.rs:138-176#briefing_residuals_phase_one_materializes_notes_before_the_first_agent_step]
